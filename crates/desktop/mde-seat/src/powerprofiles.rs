//! The power-profiles client — `net.hadess.PowerProfiles` (power-profiles-daemon)
//! over the system D-Bus (the §2 FDO-interop exception: a standard freedesktop
//! service, no MDE-private bus name).
//!
//! The daemon exposes the host's active power profile (`power-saver` / `balanced`
//! / `performance`) and the set it supports. This is the **native** seam the
//! Quasar shell reads/writes directly (design lock 2 — no mackesd Bus relay), the
//! same mechanism `powerprofilesctl` uses: read the `ActiveProfile` (`s`) and
//! `Profiles` (`aa{sv}`) properties, and switch by *writing* the `ActiveProfile`
//! property (`org.freedesktop.DBus.Properties.Set`).
//!
//! When power-profiles-daemon is not running the whole section folds to a typed
//! [`SeatError::Unavailable`] → the shell renders an honest "unavailable", never
//! a fabricated active profile (§7 / interlock 4). The name-extraction fold
//! ([`fold_profiles`]) is pure and unit-tested; the production impl does only I/O.

use crate::bus::SysBus;
use crate::error::{Backend, SeatError};
use crate::props::{str_prop, PropMap};

/// The power-profiles-daemon well-known bus name + interface.
const POWER_PROFILES: &str = "net.hadess.PowerProfiles";
/// The daemon's single object path.
const POWER_PROFILES_PATH: &str = "/net/hadess/PowerProfiles";
/// The standard FDO properties interface (get-all + writable-property set).
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// The host's power-profile state: which profile is active and which the daemon
/// offers.
///
/// An empty [`Self::available`] with the daemon present is honest (the daemon
/// reported no profiles), distinct from the section being `Absent` (daemon not
/// running at all).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileState {
    /// The active profile name (`ActiveProfile`), e.g. `balanced`.
    pub active: String,
    /// Every profile the daemon offers, in its advertised order
    /// (`power-saver` → `balanced` → `performance`).
    pub available: Vec<String>,
}

impl ProfileState {
    /// Whether `name` is one the daemon offers — the guard the drive method's
    /// caller (POWER-4) uses before a switch, so an unknown name is never sent.
    #[must_use]
    pub fn offers(&self, name: &str) -> bool {
        self.available.iter().any(|p| p == name)
    }
}

/// The power-profiles seam. Production impl: [`ZbusProfiles`]; tests inject fakes.
pub trait ProfilesClient: Send {
    /// Read the active profile + the available set.
    ///
    /// # Errors
    /// Typed: [`SeatError::Unavailable`] when power-profiles-daemon / the system
    /// bus is absent (the honest "unavailable" render).
    fn state(&self) -> Result<ProfileState, SeatError>;

    /// Switch the active profile by writing the `ActiveProfile` property.
    ///
    /// # Errors
    /// Typed: [`SeatError::Unavailable`] when the daemon / bus is absent;
    /// [`SeatError::Backend`] when the daemon refuses the name (an unknown
    /// profile) — never a silent no-op.
    fn set_active(&self, name: &str) -> Result<(), SeatError>;
}

/// The production client: `Properties.GetAll` for the read, `Properties.Set` on
/// the writable `ActiveProfile` for the switch.
pub struct ZbusProfiles {
    bus: SysBus,
}

impl ZbusProfiles {
    /// A client over the system bus. No I/O until the first call.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bus: SysBus::new(Backend::PowerProfiles),
        }
    }
}

impl Default for ZbusProfiles {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfilesClient for ZbusProfiles {
    fn state(&self) -> Result<ProfileState, SeatError> {
        let all: PropMap = self.bus.call(
            POWER_PROFILES,
            POWER_PROFILES_PATH,
            PROPERTIES,
            "GetAll",
            &(POWER_PROFILES,),
        )?;
        Ok(fold_profiles(
            str_prop(&all, "ActiveProfile"),
            &profile_dicts(&all),
        ))
    }

    fn set_active(&self, name: &str) -> Result<(), SeatError> {
        // The persistent switch is a write to the writable `ActiveProfile`
        // property (`ssv`) — the same mechanism `powerprofilesctl set` uses.
        self.bus.call_unit(
            POWER_PROFILES,
            POWER_PROFILES_PATH,
            PROPERTIES,
            "Set",
            &(
                POWER_PROFILES,
                "ActiveProfile",
                zbus::zvariant::Value::from(name),
            ),
        )
    }
}

/// Extract the `Profiles` (`aa{sv}`) property from a `GetAll` bag as the list of
/// per-profile dicts. A missing / wrongly-typed value reads as an empty list
/// (the fold then reports no available profiles) rather than a panic.
fn profile_dicts(all: &PropMap) -> Vec<PropMap> {
    all.get("Profiles")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| Vec::<PropMap>::try_from(v).ok())
        .unwrap_or_default()
}

/// Fold the daemon's `ActiveProfile` string + its `Profiles` dicts into a
/// [`ProfileState`].
///
/// Pure: each profile dict carries a `Profile` (`s`) name; blank / nameless
/// entries are dropped rather than shown as an empty choice.
#[must_use]
pub fn fold_profiles(active: Option<String>, profiles: &[PropMap]) -> ProfileState {
    let available = profiles
        .iter()
        .filter_map(|p| str_prop(p, "Profile"))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    ProfileState {
        active: active.map(|s| s.trim().to_owned()).unwrap_or_default(),
        available,
    }
}

#[cfg(test)]
mod tests {
    use zbus::zvariant::OwnedValue;

    use crate::props::testutil::{props, s};

    use super::*;

    /// One `Profiles` entry as the daemon reports it (a `Profile` name plus the
    /// `Driver`/`CpuDriver` keys the fold ignores).
    fn profile(name: &str) -> PropMap {
        props(vec![("Profile", s(name)), ("Driver", s("intel_pstate"))])
    }

    #[test]
    fn folds_the_active_profile_and_the_available_names_in_order() {
        let dicts = vec![
            profile("power-saver"),
            profile("balanced"),
            profile("performance"),
        ];
        let st = fold_profiles(Some("balanced".to_owned()), &dicts);
        assert_eq!(st.active, "balanced");
        assert_eq!(st.available, vec!["power-saver", "balanced", "performance"]);
        assert!(st.offers("performance"));
        assert!(!st.offers("turbo"));
    }

    #[test]
    fn a_present_daemon_with_no_profiles_folds_honestly_empty_not_absent() {
        // Daemon reachable (GetAll succeeded) but reported no profiles → an empty
        // available list, distinct from the section being Absent. `active` is
        // whatever the daemon said (here, missing → empty), never guessed.
        let st = fold_profiles(None, &[]);
        assert_eq!(st.active, "");
        assert!(st.available.is_empty());
        assert!(!st.offers("balanced"));
    }

    #[test]
    fn nameless_profile_entries_are_dropped_not_shown_blank() {
        let dicts = vec![
            profile("balanced"),
            props(vec![("Driver", s("placeholder"))]), // no `Profile` key
            props(vec![("Profile", s("   "))]),        // blank name
        ];
        let st = fold_profiles(Some("balanced".to_owned()), &dicts);
        assert_eq!(st.available, vec!["balanced"]);
    }

    #[test]
    fn profile_dicts_reads_the_nested_aasv_property() {
        // The prod extraction path: a GetAll bag whose `Profiles` value is the
        // real nested `aa{sv}` OwnedValue → the per-profile dict list. Proves the
        // OwnedValue → Vec<PropMap> conversion the client relies on.
        let nested: Vec<PropMap> = vec![profile("balanced"), profile("performance")];
        let all: PropMap = props(vec![
            ("ActiveProfile", s("performance")),
            (
                "Profiles",
                OwnedValue::try_from(zbus::zvariant::Value::from(nested))
                    .expect("aa{sv} → OwnedValue"),
            ),
        ]);
        let dicts = profile_dicts(&all);
        let st = fold_profiles(str_prop(&all, "ActiveProfile"), &dicts);
        assert_eq!(st.active, "performance");
        assert_eq!(st.available, vec!["balanced", "performance"]);
    }

    /// A fake over the trait seam: canned state + a recorded set target, so the
    /// drive path is exercised without a live daemon.
    struct FakeProfiles {
        state: ProfileState,
        set_to: std::sync::Mutex<Option<String>>,
    }

    impl ProfilesClient for FakeProfiles {
        fn state(&self) -> Result<ProfileState, SeatError> {
            Ok(self.state.clone())
        }
        fn set_active(&self, name: &str) -> Result<(), SeatError> {
            *self.set_to.lock().unwrap() = Some(name.to_owned());
            Ok(())
        }
    }

    #[test]
    fn the_fake_records_a_switch_target() {
        let fake = FakeProfiles {
            state: ProfileState {
                active: "balanced".into(),
                available: vec!["balanced".into(), "performance".into()],
            },
            set_to: std::sync::Mutex::new(None),
        };
        assert_eq!(fake.state().unwrap().active, "balanced");
        fake.set_active("performance").unwrap();
        assert_eq!(fake.set_to.lock().unwrap().as_deref(), Some("performance"));
    }

    #[test]
    fn the_real_client_on_this_host_answers_typed_never_panics() {
        // The build host has no power-profiles-daemon; the read must come back a
        // typed PowerProfiles-tagged error (the honest "unavailable"), never a
        // panic and never a fabricated active profile.
        match ZbusProfiles::new().state() {
            Ok(st) => {
                for p in &st.available {
                    assert!(!p.is_empty());
                }
            }
            Err(e) => assert_eq!(e.backend(), Backend::PowerProfiles),
        }
    }
}
