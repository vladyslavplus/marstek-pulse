mod app;
mod diagnostics;
mod marstek;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match diagnostics::run(&args) {
        Ok(true) => return,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
        Ok(false) => {}
    }
    if !args.is_empty() && args != ["--start-hidden"] {
        eprintln!("Unrecognized arguments");
        std::process::exit(2);
    }
    if let Err(error) = app::run(!args.is_empty()) {
        eprintln!("Application failed: {error}");
        std::process::exit(1);
    }
}
