fn main() {
    if let Err(err) = kota_v2_lib::agent_bus::run_cli() {
        eprintln!("kota-agent-bus: {err}");
        std::process::exit(1);
    }
}
