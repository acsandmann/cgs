#[path = "../examples/metal_preview.rs"]
mod demo;
fn main() -> Result<(), Box<dyn std::error::Error>> { demo::run() }
