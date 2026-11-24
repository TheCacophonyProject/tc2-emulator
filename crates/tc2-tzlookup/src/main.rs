//! Prints the name of the timezone at a location: `tc2-tzlookup <lat> <lng>`.
//!
//! Finding it means loading the world's timezone polygons, which takes about 100MB of memory,
//! and it's only needed when the location is first set or changes. Done in a process of its own
//! that exits straight afterwards, that memory goes back to the OS completely, rather than being
//! held by (or lingering in the allocator of) a long-running service. The data is bundled in
//! this binary, so it can be updated separately from tc2-agent, which is updated often.
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag] if flag == "--version" => {
            println!("tc2-tzlookup {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        [lat, lng] => match (lat.parse::<f64>(), lng.parse::<f64>()) {
            (Ok(lat), Ok(lng)) if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lng) => {
                println!("{}", tzf_rs::DefaultFinder::new().get_tz_name(lng, lat));
                ExitCode::SUCCESS
            }
            _ => {
                eprintln!("tc2-tzlookup: invalid location '{lat}' '{lng}'");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("usage: tc2-tzlookup <lat> <lng> | --version");
            ExitCode::from(2)
        }
    }
}
