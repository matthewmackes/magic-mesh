use browser_vm_production_control::hook;

fn main() {
    if let Err(error) = hook::run_probe() {
        // Collector discards hook output. Keep this bounded diagnostic free of
        // credentials, controller bodies, audio, and receipt contents anyway.
        eprintln!("browser-vm-guest-audio-probe-hook: failed closed: {error:#}");
        std::process::exit(1);
    }
}
