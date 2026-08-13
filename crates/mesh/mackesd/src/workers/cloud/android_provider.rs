//! WL-FUNC-020 S2 — production Android image/provider placement preflight.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use mackes_mesh_types::android_apps::{AndroidImagePackageManifest, AndroidSignedCatalog};
use mackes_mesh_types::android_provider::{
    AndroidProviderAdmission, AndroidProviderReadiness, AndroidProviderRefusal,
    CuttlefishImageProvenanceRef, ANDROID_PROVIDER_ADMISSION_SCHEMA_VERSION,
};
use mackes_mesh_types::cloud::{DeliveryType, WorkloadSpec};
use sha2::{Digest, Sha256};

pub(super) const ANDROID_IMAGE_FILE_ENV: &str = "MDE_ANDROID_IMAGE_FILE";
const MIN_VCPUS: u16 = 4;
const MIN_MEMORY_MIB: u64 = 8 * 1024;
const MIN_DISK_MIB: u64 = 80 * 1024;
const MAX_IMAGE_BYTES: u64 = 128 * 1024 * 1024 * 1024;
const MAX_HOST_PROBE_TEXT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AndroidHostFacts {
    pub kvm_available: bool,
    pub nested_virtualization: bool,
    pub available_vcpus: u16,
    pub available_memory_mib: u64,
    pub available_disk_mib: u64,
}

pub(super) trait AndroidHostProbe: Send + Sync {
    fn facts(&self, artifact: Option<&Path>) -> AndroidHostFacts;
    fn image_digest(&self, artifact: &Path) -> io::Result<String>;
}

#[derive(Debug, Default)]
pub(super) struct ProductionAndroidHostProbe {
    image_cache: Mutex<Option<ImageDigestCache>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageFingerprint {
    path: PathBuf,
    length: u64,
    modified_ns: u128,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
}

#[derive(Debug, Clone)]
struct ImageDigestCache {
    fingerprint: ImageFingerprint,
    digest: String,
}

impl AndroidHostProbe for ProductionAndroidHostProbe {
    fn facts(&self, artifact: Option<&Path>) -> AndroidHostFacts {
        AndroidHostFacts {
            kvm_available: kvm_available(),
            nested_virtualization: nested_virtualization_enabled(),
            available_vcpus: std::thread::available_parallelism()
                .map(|count| u16::try_from(count.get()).unwrap_or(u16::MAX))
                .unwrap_or(0),
            available_memory_mib: available_memory_mib().unwrap_or(0),
            available_disk_mib: artifact
                .and_then(|path| path.parent())
                .and_then(|path| fs2::available_space(path).ok())
                .map(|bytes| bytes / (1024 * 1024))
                .unwrap_or(0),
        }
    }

    fn image_digest(&self, artifact: &Path) -> io::Result<String> {
        let before = image_fingerprint(artifact)?;
        if let Ok(cache) = self.image_cache.lock() {
            if let Some(cache) = cache.as_ref().filter(|cache| cache.fingerprint == before) {
                return Ok(cache.digest.clone());
            }
        }
        let digest = digest_regular_file(artifact)?;
        let after = image_fingerprint(artifact)?;
        if before != after {
            return Err(io::Error::other(
                "Android image changed while its digest was computed",
            ));
        }
        let mut cache = self
            .image_cache
            .lock()
            .map_err(|_| io::Error::other("Android image digest cache is poisoned"))?;
        *cache = Some(ImageDigestCache {
            fingerprint: after,
            digest: digest.clone(),
        });
        Ok(digest)
    }
}

pub(super) struct AndroidPreflightInput<'a> {
    pub workload: &'a WorkloadSpec,
    pub catalog: Option<&'a AndroidSignedCatalog>,
    pub package_manifest: Option<&'a AndroidImagePackageManifest>,
    pub artifact: Option<&'a Path>,
    pub provider_healthy: bool,
    pub now_unix_ms: u64,
}

pub(super) fn configured_image_path() -> Option<PathBuf> {
    std::env::var_os(ANDROID_IMAGE_FILE_ENV).map(PathBuf::from)
}

pub(super) fn preflight(
    input: AndroidPreflightInput<'_>,
    probe: &dyn AndroidHostProbe,
) -> AndroidProviderAdmission {
    let facts = probe.facts(input.artifact);
    let mut provenance = None;
    let mut required_vcpus = MIN_VCPUS;
    let mut required_memory_mib = MIN_MEMORY_MIB;
    let mut required_disk_mib = MIN_DISK_MIB;

    let refusal = (|| {
        let catalog = input
            .catalog
            .ok_or(AndroidProviderRefusal::CatalogUnavailable)?;
        catalog
            .payload
            .validate()
            .map_err(|_| AndroidProviderRefusal::CatalogUnavailable)?;
        if input.now_unix_ms < catalog.payload.issued_at_unix_ms {
            return Err(AndroidProviderRefusal::CatalogUnavailable);
        }
        if input.now_unix_ms > catalog.payload.expires_at_unix_ms {
            return Err(AndroidProviderRefusal::CatalogExpired);
        }
        let image = &catalog.payload.image_manifest;
        provenance = CuttlefishImageProvenanceRef::new(
            image.image_id.clone(),
            image.image_digest.clone(),
            image.source_revision.clone(),
            image.catalog_revision.clone(),
        )
        .ok();
        if provenance.is_none() {
            return Err(AndroidProviderRefusal::CatalogImageMismatch);
        }

        for policy in &catalog.payload.app_policies {
            required_vcpus = required_vcpus.max(u16::from(policy.resources.vcpus));
            required_memory_mib = required_memory_mib.max(u64::from(policy.resources.memory_mib));
            required_disk_mib = required_disk_mib.max(u64::from(policy.resources.disk_mib));
        }

        let package_manifest = input
            .package_manifest
            .ok_or(AndroidProviderRefusal::PackageManifestUnavailable)?;
        if package_manifest != &catalog.payload.package_manifest {
            return Err(AndroidProviderRefusal::PackageManifestMismatch);
        }
        if input.workload.delivery_type != DeliveryType::AndroidVm
            || !input.workload.network_isolation
            || input.workload.image.as_deref() != Some(image.image_id.as_str())
            || input.workload.image_digest.as_deref() != Some(image.image_digest.as_str())
            || u64::from(input.workload.vcpu) < required_vcpus.into()
            || u64::from(input.workload.memory_mb) < required_memory_mib
            || u64::from(input.workload.disk_gb) * 1024 < required_disk_mib
        {
            return Err(AndroidProviderRefusal::DesiredImageMismatch);
        }
        let artifact = input
            .artifact
            .ok_or(AndroidProviderRefusal::ImageArtifactUnavailable)?;
        let digest = probe
            .image_digest(artifact)
            .map_err(|_| AndroidProviderRefusal::ImageArtifactUnavailable)?;
        if digest != image.image_digest {
            return Err(AndroidProviderRefusal::ImageDigestMismatch);
        }
        if !facts.kvm_available {
            return Err(AndroidProviderRefusal::KvmUnavailable);
        }
        if !facts.nested_virtualization {
            return Err(AndroidProviderRefusal::NestedVirtualizationUnavailable);
        }
        if facts.available_vcpus < required_vcpus {
            return Err(AndroidProviderRefusal::InsufficientVcpu);
        }
        if facts.available_memory_mib < required_memory_mib {
            return Err(AndroidProviderRefusal::InsufficientMemory);
        }
        if facts.available_disk_mib < required_disk_mib {
            return Err(AndroidProviderRefusal::InsufficientDisk);
        }
        if !input.provider_healthy {
            return Err(AndroidProviderRefusal::ProviderUnavailable);
        }
        Ok(())
    })()
    .err();

    let admission = AndroidProviderAdmission {
        schema_version: ANDROID_PROVIDER_ADMISSION_SCHEMA_VERSION,
        workload_id: input.workload.name.clone(),
        image_provenance: provenance,
        readiness: if refusal.is_none() {
            AndroidProviderReadiness::Ready
        } else {
            AndroidProviderReadiness::Unavailable
        },
        refusal,
        kvm_available: facts.kvm_available,
        nested_virtualization: facts.nested_virtualization,
        provider_healthy: input.provider_healthy,
        required_vcpus,
        available_vcpus: facts.available_vcpus,
        required_memory_mib,
        available_memory_mib: facts.available_memory_mib,
        required_disk_mib,
        available_disk_mib: facts.available_disk_mib,
        observed_at_unix_ms: input.now_unix_ms.max(1),
    };
    debug_assert!(admission.validate().is_ok());
    admission
}

fn kvm_available() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        fs::symlink_metadata("/dev/kvm")
            .map(|metadata| metadata.file_type().is_char_device())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn nested_virtualization_enabled() -> bool {
    [
        "/sys/module/kvm_intel/parameters/nested",
        "/sys/module/kvm_amd/parameters/nested",
    ]
    .iter()
    .filter_map(|path| read_bounded_host_text(path).ok())
    .map(|value| value.trim().to_ascii_lowercase())
    .any(|value| matches!(value.as_str(), "1" | "y" | "yes"))
}

fn available_memory_mib() -> Option<u64> {
    read_bounded_host_text("/proc/meminfo")
        .ok()?
        .lines()
        .find_map(|line| {
            let value = line.strip_prefix("MemAvailable:")?;
            value.split_whitespace().next()?.parse::<u64>().ok()
        })
        .map(|kib| kib / 1024)
}

fn read_bounded_host_text(path: impl AsRef<Path>) -> io::Result<String> {
    let path = path.as_ref();
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_HOST_PROBE_TEXT_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "host probe text is not a bounded regular file",
        ));
    }
    let file = File::open(path)?;
    let mut body = String::new();
    file.take((MAX_HOST_PROBE_TEXT_BYTES + 1) as u64)
        .read_to_string(&mut body)?;
    if body.len() > MAX_HOST_PROBE_TEXT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "host probe text exceeds its byte limit",
        ));
    }
    Ok(body)
}

fn digest_regular_file(path: &Path) -> io::Result<String> {
    let before = fs::symlink_metadata(path)?;
    if !before.file_type().is_file() || before.len() == 0 || before.len() > MAX_IMAGE_BYTES {
        return Err(io::Error::other(
            "Android image is not an admitted regular file",
        ));
    }
    let mut file = File::open(path)?;
    let opened = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != opened.dev() || before.ino() != opened.ino() {
            return Err(io::Error::other("Android image changed during admission"));
        }
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn image_fingerprint(path: &Path) -> io::Result<ImageFingerprint> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > MAX_IMAGE_BYTES {
        return Err(io::Error::other(
            "Android image is not an admitted regular file",
        ));
    }
    let modified_ns = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| io::Error::other("Android image modification time predates Unix epoch"))?
        .as_nanos();
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(ImageFingerprint {
        path: path.to_path_buf(),
        length: metadata.len(),
        modified_ns,
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(unix)]
        changed_seconds: metadata.ctime(),
        #[cfg(unix)]
        changed_nanoseconds: metadata.ctime_nsec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use mackes_mesh_types::android_apps::{
        AndroidAppCapability, AndroidAppPermission, AndroidCatalogAppPolicy,
        AndroidCatalogGuestReadiness, AndroidCatalogPayload, AndroidImageManifest,
        AndroidImagePackage, AndroidImageProvenance, AndroidPackageVersion, AndroidResourceClass,
        AndroidResourceProfile, AospStarterApp, AospStarterCatalog,
        ANDROID_SIGNED_CATALOG_SCHEMA_VERSION,
    };
    use mackes_mesh_types::cloud::StoragePool;

    const NOW: u64 = 1_800_000_000_000;
    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    struct FakeProbe {
        facts: AndroidHostFacts,
        digest: io::Result<String>,
    }

    impl AndroidHostProbe for FakeProbe {
        fn facts(&self, _artifact: Option<&Path>) -> AndroidHostFacts {
            self.facts
        }

        fn image_digest(&self, _artifact: &Path) -> io::Result<String> {
            self.digest
                .as_ref()
                .map(Clone::clone)
                .map_err(|error| io::Error::new(error.kind(), error.to_string()))
        }
    }

    fn catalog() -> AndroidSignedCatalog {
        let image = AndroidImageManifest::new(
            "aosp-cuttlefish-production",
            DIGEST,
            "aosp-source-r1",
            "starter-r1",
            NOW - 2_000,
            NOW - 1_000,
            AospStarterCatalog::v1(),
        )
        .unwrap();
        let provenance = AndroidImageProvenance::from_manifest(&image).unwrap();
        let packages = AospStarterApp::ALL
            .into_iter()
            .map(|app| {
                AndroidImagePackage::for_app(
                    app,
                    AndroidPackageVersion::new("2026.08.1", 1).unwrap(),
                )
            })
            .collect();
        let package_manifest = AndroidImagePackageManifest::new(provenance, packages).unwrap();
        let app_policies = AospStarterApp::ALL
            .into_iter()
            .map(|app| AndroidCatalogAppPolicy {
                app,
                permissions: vec![AndroidAppPermission::Network],
                capabilities: vec![AndroidAppCapability::VdiDisplay],
                resources: AndroidResourceProfile {
                    class: AndroidResourceClass::Standard,
                    vcpus: 4,
                    memory_mib: 4_096,
                    disk_mib: 16_384,
                },
                guest_readiness: AndroidCatalogGuestReadiness::BootedInventoryAndLauncherReady,
            })
            .collect();
        AndroidSignedCatalog::sign(
            "android-release-v1",
            AndroidCatalogPayload {
                schema_version: ANDROID_SIGNED_CATALOG_SCHEMA_VERSION,
                catalog_id: "aosp-production".into(),
                revision: 1,
                issued_at_unix_ms: NOW - 2_000,
                expires_at_unix_ms: NOW + 60_000,
                image_manifest: image,
                package_manifest,
                app_policies,
            },
            &SigningKey::from_bytes(&[7; 32]),
        )
        .unwrap()
    }

    fn workload() -> WorkloadSpec {
        WorkloadSpec {
            name: "android-phone".into(),
            delivery_type: DeliveryType::AndroidVm,
            node: "bigboy".into(),
            vcpu: 4,
            memory_mb: 8_192,
            disk_gb: 80,
            storage_pool: StoragePool::LocalXfs,
            image: Some("aosp-cuttlefish-production".into()),
            image_digest: Some(DIGEST.into()),
            network_isolation: true,
            raw_hcl: None,
            app: None,
        }
    }

    fn healthy_probe() -> FakeProbe {
        FakeProbe {
            facts: AndroidHostFacts {
                kvm_available: true,
                nested_virtualization: true,
                available_vcpus: 16,
                available_memory_mib: 32_768,
                available_disk_mib: 256 * 1_024,
            },
            digest: Ok(DIGEST.into()),
        }
    }

    fn run(
        catalog: &AndroidSignedCatalog,
        probe: &FakeProbe,
        now: u64,
        healthy: bool,
    ) -> AndroidProviderAdmission {
        run_with_workload(&workload(), catalog, probe, now, healthy)
    }

    fn run_with_workload(
        workload: &WorkloadSpec,
        catalog: &AndroidSignedCatalog,
        probe: &FakeProbe,
        now: u64,
        healthy: bool,
    ) -> AndroidProviderAdmission {
        preflight(
            AndroidPreflightInput {
                workload,
                catalog: Some(catalog),
                package_manifest: Some(&catalog.payload.package_manifest),
                artifact: Some(Path::new("/image.qcow2")),
                provider_healthy: healthy,
                now_unix_ms: now,
            },
            probe,
        )
    }

    #[test]
    fn complete_signed_image_host_capacity_and_provider_health_are_required_for_ready() {
        let admission = run(&catalog(), &healthy_probe(), NOW, true);
        assert!(admission.is_ready());
        assert_eq!(admission.refusal, None);
        assert_eq!(admission.required_disk_mib, 80 * 1024);
    }

    #[test]
    fn preflight_matrix_refuses_stale_digest_nested_capacity_and_provider_failures() {
        let signed = catalog();
        let mut cases = Vec::new();
        cases.push((
            run(&signed, &healthy_probe(), NOW + 60_001, true),
            AndroidProviderRefusal::CatalogExpired,
        ));
        let mut digest = healthy_probe();
        digest.digest = Ok(format!("sha256:{}", "f".repeat(64)));
        cases.push((
            run(&signed, &digest, NOW, true),
            AndroidProviderRefusal::ImageDigestMismatch,
        ));
        let mut nested = healthy_probe();
        nested.facts.nested_virtualization = false;
        cases.push((
            run(&signed, &nested, NOW, true),
            AndroidProviderRefusal::NestedVirtualizationUnavailable,
        ));
        let mut capacity = healthy_probe();
        capacity.facts.available_memory_mib = 4_096;
        cases.push((
            run(&signed, &capacity, NOW, true),
            AndroidProviderRefusal::InsufficientMemory,
        ));
        cases.push((
            run(&signed, &healthy_probe(), NOW, false),
            AndroidProviderRefusal::ProviderUnavailable,
        ));
        for (admission, reason) in cases {
            assert!(!admission.is_ready());
            assert_eq!(admission.refusal, Some(reason));
            assert_eq!(admission.readiness, AndroidProviderReadiness::Unavailable);
        }
    }

    #[test]
    fn future_issued_catalog_is_not_admitted_before_validity_window() {
        let mut future = catalog();
        future.payload.issued_at_unix_ms = NOW + 1;

        let admission = run(&future, &healthy_probe(), NOW, true);

        assert!(!admission.is_ready());
        assert_eq!(
            admission.refusal,
            Some(AndroidProviderRefusal::CatalogUnavailable)
        );
        assert_eq!(admission.readiness, AndroidProviderReadiness::Unavailable);
    }

    #[test]
    fn provider_refuses_non_android_or_non_isolated_workload_replay() {
        let signed = catalog();
        let mut hostile = workload();
        hostile.delivery_type = DeliveryType::ServiceContainer;
        let wrong_class = run_with_workload(&hostile, &signed, &healthy_probe(), NOW, true);

        hostile.delivery_type = DeliveryType::AndroidVm;
        hostile.network_isolation = false;
        let unisolated = run_with_workload(&hostile, &signed, &healthy_probe(), NOW, true);

        for admission in [wrong_class, unisolated] {
            assert_eq!(
                admission.refusal,
                Some(AndroidProviderRefusal::DesiredImageMismatch)
            );
            assert_eq!(admission.readiness, AndroidProviderReadiness::Unavailable);
        }
    }

    #[test]
    fn oversized_host_probe_text_is_rejected_before_parse() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("probe");
        std::fs::write(&path, vec![b'x'; MAX_HOST_PROBE_TEXT_BYTES + 1]).unwrap();
        assert_eq!(
            read_bounded_host_text(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn production_image_probe_hashes_regular_files_and_refuses_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("android.img");
        fs::write(&image, b"immutable android image").unwrap();
        let probe = ProductionAndroidHostProbe::default();
        let digest = probe.image_digest(&image).unwrap();
        assert_eq!(
            digest,
            format!("sha256:{:x}", Sha256::digest(b"immutable android image"))
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&image, temp.path().join("link.img")).unwrap();
            assert!(probe.image_digest(&temp.path().join("link.img")).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn production_image_probe_invalidates_cache_after_same_inode_rewrite() {
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("android.img");
        fs::write(&image, b"signed-image-one").unwrap();
        let original_metadata = fs::metadata(&image).unwrap();
        let original_modified = original_metadata.modified().unwrap();
        let original_accessed = original_metadata.accessed().unwrap();
        let probe = ProductionAndroidHostProbe::default();
        let original_digest = probe.image_digest(&image).unwrap();

        // Preserve every attacker-controlled cache key: path, inode, length,
        // and mtime.  Linux ctime still advances for this in-place rewrite and
        // must invalidate the previously admitted signed-image digest.
        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(&image, b"hostile-image-two").unwrap();
        File::options()
            .write(true)
            .open(&image)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_accessed(original_accessed)
                    .set_modified(original_modified),
            )
            .unwrap();

        let replacement_digest = probe.image_digest(&image).unwrap();
        assert_ne!(replacement_digest, original_digest);
        assert_eq!(
            replacement_digest,
            format!("sha256:{:x}", Sha256::digest(b"hostile-image-two"))
        );
    }
}
