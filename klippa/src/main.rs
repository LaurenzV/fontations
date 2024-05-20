//! binary subset tool
//!
//! Takes a font file and a subset input which describes the desired subset, and ouput is a new
//! font file containing only the data specified in the input.
//!

use clap::Parser;
use klippa::{parse_unicodes, populate_gids, subset_font, Plan};
use write_fonts::read::FontRef;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The input font file.
    #[arg(short, long)]
    path: std::path::PathBuf,

    /// List of glyph ids
    #[arg(short, long)]
    gids: Option<String>,

    /// List of unicode codepoints
    #[arg(short, long)]
    unicodes: Option<String>,

    /// The output font file
    #[arg(short, long)]
    output_file: std::path::PathBuf,
}

fn main() {
    let args = Args::parse();

    let gids = match populate_gids(&args.gids.unwrap_or_default()) {
        Ok(gids) => gids,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let unicodes = match parse_unicodes(&args.unicodes.unwrap_or_default()) {
        Ok(unicodes) => unicodes,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let font_bytes = std::fs::read(&args.path).expect("Invalid input font file found");
    let font = FontRef::new(&font_bytes).expect("Error reading font bytes");
    let plan = Plan::new(&gids, &unicodes, &font);

    subset_font(font, &plan, &args.output_file);
}
