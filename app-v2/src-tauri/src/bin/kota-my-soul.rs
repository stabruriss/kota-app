fn main() {
    if let Err(err) = kota_v2_lib::soul::run_cli() {
        eprintln!("kota-my-soul: {err}");
        std::process::exit(1);
    }
}
