use browser_vm_production_control::hook;

fn main() {
    if let Err(error) = hook::run_reconnect() {
        eprintln!("browser-vm-reconnect-hook: failed closed: {error:#}");
        std::process::exit(1);
    }
}
