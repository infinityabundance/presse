use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

use crate::transcode::Acceleration;

/// Fast PDF compression tool - easier and faster than ghostscript
#[derive(Parser)]
#[command(name = "presse")]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Compress one or several PDF documents
    Press {
        /// Input file
        input: Vec<PathBuf>,

        /// Output file (optional, defaults to <input>_compressed.pdf)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Target quality for lossy image compression
        #[arg(short, long, default_value_t = 80)]
        quality: u8,

        /// Image transcoding backend: cpu (default), auto, cuda, or rocm.
        /// The gpu backends require a build with --features cuda / rocm.
        #[arg(short, long, value_enum, default_value_t = Acceleration::Cpu)]
        acceleration: Acceleration,

        /// Cap the effective resolution of placed images to DPI pixels per
        /// inch. Omitted: images keep their source resolution.
        #[arg(
            short = 'd',
            long,
            value_name = "DPI",
            value_parser = clap::value_parser!(u32).range(1..),
            long_help = "Cap the effective resolution of placed images to DPI pixels per inch.\n\nAn image drawn at w×h points is downsampled to at most w·DPI/72 × h·DPI/72\npixels (the same placement-aware rule Ghostscript's -dPDFSETTINGS uses). The\ncap is strict: it never up-samples, and images whose on-page placement\ncannot be determined keep their source resolution.\n\nPresets:\n  75   screen     – low resolution\n  150  ebook      – medium resolution\n  300  printer    – high quality\n  600  prepress   – high quality, color preserving\n\nDefault (flag omitted): no downsampling, current behavior.",
        )]
        dpi: Option<u32>,

        // Details during the compression process --> sizes comparison before & after
        #[arg(short, long, default_value_t = false)]
        verbose: bool,
    },

    Merge {
        /// Input files (>= 2, order matters)
        input: Vec<PathBuf>,

        /// Output file (optional, defaults to <input>_merged.pdf)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Compress the merged file
        #[arg(short, long, default_value_t = false)]
        compress: bool,
    },

    /// Convert one or several images to PDF
    Convert {
        /// Input image files
        input: Vec<PathBuf>,

        /// Output file or directory (optional)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Merge all images into a single PDF
        #[arg(short, long, default_value_t = false)]
        merge: bool,

        /// JPEG-compress the embedded images
        #[arg(short, long, default_value_t = false)]
        compress: bool,

        /// Target quality for --compress (1-100)
        #[arg(short, long, default_value_t = 80)]
        quality: u8,

        /// Verbose output
        #[arg(short, long, default_value_t = false)]
        verbose: bool,
    },
}

pub fn resolve_press_path_output(file_path: &Path, output: &Option<PathBuf>) -> PathBuf {
    match output {
        Some(path) if path.is_dir() || path.to_str().unwrap().ends_with('/') => {
            let stem = file_path.file_stem().unwrap().to_str().unwrap();
            path.join(format!("{}_compressed.pdf", stem))
        }
        Some(path) => path.clone(),
        None => {
            let stem = file_path.file_stem().unwrap().to_str().unwrap();
            let mut path = file_path.to_path_buf();
            path.set_file_name(format!("{}_compressed.pdf", stem));
            path
        }
    }
}

pub fn resolve_convert_path_output(file_path: &Path, output: &Option<PathBuf>) -> PathBuf {
    match output {
        Some(path) if path.is_dir() || path.to_str().unwrap().ends_with('/') => {
            let stem = file_path.file_stem().unwrap().to_str().unwrap();
            path.join(format!("{}.pdf", stem))
        }
        Some(path) => path.clone(),
        None => file_path.with_extension("pdf"),
    }
}

pub fn resolve_merge_path_output(output: &Option<PathBuf>, compress: bool) -> PathBuf {
    let default_name = if compress {
        "compressed_merged.pdf"
    } else {
        "merged.pdf"
    };
    match output {
        Some(path) if path.is_dir() || path.to_str().unwrap().ends_with('/') => {
            path.join(default_name)
        }
        Some(path) => path.clone(),
        None => PathBuf::from(default_name),
    }
}
