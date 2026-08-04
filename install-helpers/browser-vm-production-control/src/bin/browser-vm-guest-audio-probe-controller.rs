use browser_vm_production_control::controller;

fn main() {
    if let Err(error) = controller::run() {
        eprintln!("browser-vm-guest-audio-probe-controller: failed: {error:#}");
        std::process::exit(1);
    }
}
