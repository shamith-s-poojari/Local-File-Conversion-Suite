//! Prints what the sniffer makes of each file: `cargo run -p converter-sniffer
//! --example sniff -- FILE...`. A development aid for checking that adapter
//! outputs sniff as their declared format (the core refuses them otherwise).

use std::path::Path;

fn main() {
    for arg in std::env::args().skip(1) {
        match converter_sniffer::sniff_path(Path::new(&arg)) {
            Ok(d) => println!("{}\t{:?}\t{:?}", arg, d.format, d.candidates),
            Err(e) => println!("{arg}\terror\t{e:?}"),
        }
    }
}
