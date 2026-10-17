//! greycard: decode a RAW, white balance and color-transform it correctly,
//! write the result somewhere every editor can read.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use greycard_core::output::OnExists;
use greycard_core::register::Model;
use greycard_core::stack::Reference;

mod common;
mod develop;
mod info;
mod lenses;
mod library;
mod log;
mod models;
mod presets;
mod rawtherapee;
mod register;
mod stack;

#[derive(Parser)]
#[command(
    name = "greycard",
    version,
    about = "RAW pre-processor: decode, white balance and camera color, done right"
)]
struct Cli {
    /// Say more: once, what greycard's own crates note along the way;
    /// twice, their debug lines. RUST_LOG, when set, decides instead
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
// Develop carries every setting; the size gap to Info is of no consequence.
#[allow(clippy::large_enum_variant)]
enum Command {
    /// Print what the file says about itself and what the engine makes of it
    Info { file: PathBuf },
    /// The lens database: where it is and what it has; with a file,
    /// what it finds for that file
    Lenses {
        file: Option<PathBuf>,
        /// Fetch the database (about 450 KB, CC BY-SA 3.0) into the
        /// user's cache first
        #[arg(long)]
        fetch: bool,
        /// Look a lens up by name instead of from a file (with
        /// --camera for the body)
        #[arg(long, value_name = "NAME")]
        lens: Option<String>,
        /// The body to look a lens up for, as make and model
        #[arg(long, num_args = 2, value_names = ["MAKE", "MODEL"])]
        camera: Option<Vec<String>>,
    },
    /// The learned models: where they are kept, what each is for, and
    /// what it costs to fetch one
    Models {
        /// Fetch a denoiser tier (fast, balanced, best), a model by
        /// its id, or `all`, into the user's model store. Each
        /// model's license is printed before it is fetched, and the
        /// published hash is checked when it arrives
        #[arg(long, value_name = "TIER|ID|all")]
        fetch: Option<String>,
    },
    /// The library: an index of folders' files, their EXIF and their
    /// sidecars' meta, and a listing of it by filter
    Library {
        /// A database other than the user's (greycard/library.sqlite
        /// under the platform's data directory)
        #[arg(long, value_name = "FILE", global = true)]
        db: Option<PathBuf>,
        #[command(subcommand)]
        action: library::LibraryAction,
    },
    /// The presets: list them; import Lightroom (.xmp) or greycard
    /// (.gcp) preset files; lay one over files' edits
    Presets {
        /// Files whose sidecars take the preset named by --apply; with
        /// no --apply, the versions each sidecar holds are listed
        files: Vec<PathBuf>,
        /// Preset files to bring into the store, Lightroom's .xmp or
        /// this engine's .gcp; a name the store has already is refused
        /// without --replace
        #[arg(long, value_name = "FILE", num_args = 1..)]
        import: Vec<PathBuf>,
        /// With --import, write over a preset of the same name rather
        /// than refuse the file
        #[arg(long, requires = "import")]
        replace: bool,
        /// Lay this preset (by name, or a preset file's path) over
        /// each file's edit, as a step in its history
        #[arg(long, value_name = "NAME")]
        apply: Option<String>,
        /// A store other than the user's
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
    /// Import RawTherapee's .pp3 profiles: for each raw, the profile
    /// beside it (IMG_0001.CR3.pp3, or IMG_0001.pp3) read, its rank,
    /// color label and trash mark carried into the meta exactly, and
    /// its white balance, exposure, geometry and lens settings, as near
    /// as this engine has them, laid over the edit as one state named
    /// for the import. What has no place here is named. A dry run
    /// unless --write
    Rawtherapee {
        /// Raws, or folders whose raws (not their subfolders') to read
        paths: Vec<PathBuf>,
        /// Write the sidecars; without it, only the report
        #[arg(long)]
        write: bool,
        /// Import into a frame that has a history here already, as a
        /// new state on top of it. Forced, every setting the profile's
        /// read sections hold comes across, RawTherapee's neutral ones
        /// too (an exposure of 0 sets this frame's back to 0), so the
        /// frame follows RawTherapee; the report lists those as neutral
        #[arg(long)]
        force: bool,
    },
    /// Fit the transform that puts one frame on top of another:
    /// pyramidal Lucas-Kanade on the two frames' luminance, for a
    /// stack's alignment. Prints the transform, what it moves the
    /// frame by, and the residual that says whether to believe it
    Register {
        /// The frame everything else is put onto
        reference: PathBuf,
        /// The frame whose transform onto the reference is wanted
        moving: PathBuf,
        /// What to fit: translation, similarity (shift, rotation and
        /// one scale) or affine
        #[arg(long, default_value = "similarity")]
        model: Model,
        /// Stop at this pyramid level rather than at full resolution:
        /// 1 is half size, 2 a quarter. Halves the time and costs a
        /// fraction of a pixel
        #[arg(long, default_value_t = 0)]
        finest_level: usize,
    },
    /// Merge a focus stack: register the frames onto one another, take
    /// each band of the picture from whichever frame holds it
    /// sharpest, and write the frame that is sharp everywhere beside
    /// the sources. Prints what each frame contributed and what was
    /// left out.
    ///
    /// Every frame is held in memory at once: about 0.6 s and a
    /// gigabyte a frame at 24 megapixels, and ten 45-megapixel frames
    /// peak at 9.3 GB
    Stack {
        /// The frames, in focus order. RAW files of one camera and one
        /// size
        files: Vec<PathBuf>,
        /// Where the merge goes: a linear DNG (.dng) for any editor to
        /// finish, or a 16-bit linear Rec.2020 TIFF (.tif). Without
        /// one, a DNG beside the first frame and named after it — the
        /// name comes from the first frame, the color tags and the
        /// EXIF from the reference
        #[arg(short, long, value_name = "OUT.dng")]
        output: Option<PathBuf>,
        /// The frame the others are put onto: `middle` (the one
        /// nearest in magnification to all the rest, since focus
        /// breathing runs one way through a stack), `sharpest`, or a
        /// frame's number counting from 0
        #[arg(long, default_value = "middle", value_parser = stack::stack_reference)]
        reference: Reference,
        /// How far each band is taken from the frame with the most to
        /// say in it rather than averaged: 0 is a weighted average,
        /// higher keeps more detail and less of the stack's noise
        /// averaging
        #[arg(long, default_value_t = greycard_core::stack::Options::default().selectivity)]
        selectivity: f32,
        /// The residual a link between two neighboring frames may
        /// have and still be believed. Frames of a focus stack differ
        /// by their defocus, so this is not the registration's own
        /// threshold. The default is measured on synthetic frames and
        /// on two real scenes, not on a real focus stack
        #[arg(long, default_value_t = greycard_core::stack::Options::default().max_residual)]
        max_residual: f32,
        /// Store the DNG samples uncompressed instead of lossless JPEG
        #[arg(long)]
        uncompressed: bool,
        /// What to do when the output file is there already
        #[arg(long, value_name = "WHAT", default_value = "increment",
              value_parser = common::on_exists_named)]
        on_exists: OnExists,
    },
    /// Develop a RAW: linear DNG for other editors, or linear Rec.2020 here
    Develop(develop::DevelopArgs),
}

/// The command, and then the process's end: by `runtime::leave`, so a
/// run that opened a model ends without ONNX Runtime's exit handler,
/// which could crash after the output was written (exit 139 to a
/// script). An error is said as `main` returning it would say it.
fn main() -> std::process::ExitCode {
    let code = match run() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {e:?}");
            1
        }
    };
    greycard_ai::runtime::leave(code)
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    log::start(cli.verbose);
    // A panic inside rawler is caught and comes back as the file's
    // decode error, which is printed: its own print would say the
    // same thing twice, the first time as if it were a crash.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !greycard_core::decode::inside_decoder() {
            hook(info);
        }
    }));
    match cli.command {
        Command::Library { db, action } => library::library(db, action),
        Command::Info { file } => {
            if greycard_core::picture::is_picture_path(&file) {
                info::info_picture(&file)
            } else {
                info::info(&file)
            }
        }
        Command::Lenses {
            file,
            fetch,
            lens,
            camera,
        } => lenses::lenses(file.as_deref(), fetch, lens.as_deref(), camera.as_deref()),
        Command::Models { fetch } => models::models(fetch.as_deref()),
        Command::Rawtherapee {
            paths,
            write,
            force,
        } => rawtherapee::rawtherapee(&paths, write, force),
        Command::Register {
            reference,
            moving,
            model,
            finest_level,
        } => register::register(&reference, &moving, model, finest_level),
        Command::Stack {
            files,
            output,
            reference,
            selectivity,
            max_residual,
            uncompressed,
            on_exists,
        } => stack::stack_files(
            &files,
            output.as_deref(),
            reference,
            selectivity,
            max_residual,
            uncompressed,
            on_exists,
        ),
        Command::Presets {
            files,
            import,
            replace,
            apply,
            dir,
        } => presets::presets(&files, &import, replace, apply.as_deref(), dir),
        Command::Develop(args) => develop::run(args),
    }
}
