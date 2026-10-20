fn main() {
    if let Err(error) = kota_v2_lib::dream_entry::run_cli() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
