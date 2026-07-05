fn main() {
    if let Err(err) = sv_to_sexpr::run() {
        eprintln!("{}", err);
        std::process::exit(1);
    }
}
