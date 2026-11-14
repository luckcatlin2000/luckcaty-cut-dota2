use std::{env, path::PathBuf};

fn main() {
    let args: Vec<_> = env::args_os().skip(1).map(PathBuf::from).collect();
    if args.len() != 4 {
        eprintln!("Usage: import_timestamp_plan REPLAY PLAN JOBS_ROOT REPORT");
        std::process::exit(2);
    }
    match d2_highlights_desktop_lib::import_timestamp_file(&args[0], &args[1], &args[2]) {
        Ok(report) => {
            d2_highlights_core::write_json_pretty(&args[3], &report)
                .expect("cannot write import receipt");
            println!("{}", report);
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
