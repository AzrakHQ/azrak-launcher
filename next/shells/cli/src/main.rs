fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let app = match launcher::App::open(launcher::default_home()) {
        Ok(a) => a,
        Err(e) => { eprintln!("error: {e}"); std::process::exit(2); }
    };
    let mut out = std::io::stdout();
    if let Err(e) = launcher::run(&args, &app, &mut out) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
