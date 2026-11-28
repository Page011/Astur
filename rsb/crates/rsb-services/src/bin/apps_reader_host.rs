//! A stand-in host for the Start menu reader. The apps service starts its own program again with
//! `--apps-reader`; a test of the service runs inside a test program that cannot do that, so it
//! points the service here instead (`RSB_APPS_READER_EXE`).

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some(rsb_services::apps_reader::FLAG) {
        eprintln!("apps_reader_host: only {} is understood", rsb_services::apps_reader::FLAG);
        return std::process::ExitCode::from(2);
    }
    std::process::ExitCode::from(rsb_services::apps_reader::main(&args[1..]) as u8)
}
