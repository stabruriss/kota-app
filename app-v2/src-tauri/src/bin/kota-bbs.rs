fn main() {
    if let Err(err) = kota_v2_lib::bbs::run_cli() {
        eprintln!("kota-bbs: {err}");
        std::process::exit(1);
    }
}
