use std::env;

fn main() {
    // Linker-Konfiguration für ATtiny45
    let target = env::var("TARGET").unwrap_or_default();

    if target.contains("avr") {
        println!("cargo:rustc-link-arg=-mmcu=attiny45");
    }
}
