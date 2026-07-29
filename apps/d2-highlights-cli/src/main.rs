use clap::{Parser, Subcommand};
use d2_highlights_core::{ingest_dem, write_json_pretty};
use d2_highlights_detector::detect_highlights;
use d2_highlights_director::build_director_plan;
use d2_highlights_parser_source2::parse_combat_timeline;
use d2_highlights_pipeline::analyze_dem;
use d2_highlights_replay_control::{
    build_replay_control_plan, execute_vconsole_commands, probe_vconsole,
};
use serde::Serialize;
use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "d2-highlights")]
#[command(about = "Local Dota 2 replay analysis pipeline")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Validate a DEM and create an idempotent job manifest without copying it.
    Ingest {
        dem: PathBuf,
        #[arg(long, default_value = "jobs")]
        jobs_dir: PathBuf,
    },
    /// Parse Dota 2 combat log events into the project timeline schema.
    Parse {
        dem: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Score combat clusters and write explainable highlight candidates.
    Detect {
        timeline: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value_t = 20)]
        max_candidates: usize,
        #[arg(long, default_value_t = 18.0)]
        min_score: f32,
    },
    /// Turn ranked highlights into a deterministic camera and audio plan.
    Direct {
        highlights: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value = "comic_hype_v1")]
        template: String,
        #[arg(long, default_value_t = 10)]
        max_clips: usize,
        #[arg(long, default_value_t = 90.0)]
        max_duration: f32,
    },
    /// Convert a director plan into an auditable tick-level replay control plan.
    ControlPlan {
        director: PathBuf,
        timeline: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Verify the current Dota 2 VConsole handshake using a harmless echo command.
    VconsoleProbe {
        #[arg(long, default_value_t = 5)]
        timeout_seconds: u64,
    },
    /// Send one or more allowlisted commands to a locally running offline replay.
    VconsoleExec {
        #[arg(long = "command", required = true)]
        commands: Vec<String>,
        #[arg(long, default_value_t = 8)]
        timeout_seconds: u64,
    },
    /// Run ingest and combat-log parsing as one resumable job.
    Analyze {
        dem: PathBuf,
        #[arg(long, default_value = "jobs")]
        jobs_dir: PathBuf,
    },
    /// Report the local tools needed by later pipeline stages.
    Doctor,
}

#[derive(Serialize)]
struct ToolCheck {
    name: &'static str,
    found: bool,
    path: Option<String>,
}

#[derive(Serialize)]
struct DoctorReport {
    ffmpeg: ToolCheck,
    ffprobe: ToolCheck,
    dota2: ToolCheck,
    vconsole2: ToolCheck,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Ingest { dem, jobs_dir } => {
            let result = ingest_dem(&dem, &jobs_dir)?;
            println!("{}", serde_json::to_string_pretty(&result.manifest)?);
        }
        Commands::Parse { dem, output } => {
            let source = d2_highlights_core::inspect_dem(&dem)?;
            let timeline = parse_combat_timeline(&dem, &source.sha256)?;
            write_json_pretty(&output, &timeline)?;
            println!("{}", output.canonicalize()?.display());
        }
        Commands::Detect {
            timeline,
            output,
            max_candidates,
            min_score,
        } => {
            let timeline = serde_json::from_slice::<d2_highlights_core::TimelineDocument>(
                &std::fs::read(timeline)?,
            )?;
            let highlights = detect_highlights(&timeline, max_candidates, min_score);
            write_json_pretty(&output, &highlights)?;
            println!("{}", output.canonicalize()?.display());
        }
        Commands::Direct {
            highlights,
            output,
            template,
            max_clips,
            max_duration,
        } => {
            let highlights = serde_json::from_slice::<d2_highlights_core::HighlightDocument>(
                &std::fs::read(highlights)?,
            )?;
            let director = build_director_plan(&highlights, &template, max_clips, max_duration);
            write_json_pretty(&output, &director)?;
            println!("{}", output.canonicalize()?.display());
        }
        Commands::ControlPlan {
            director,
            timeline,
            output,
        } => {
            let director = serde_json::from_slice::<d2_highlights_core::DirectorDocument>(
                &std::fs::read(director)?,
            )?;
            let timeline = serde_json::from_slice::<d2_highlights_core::TimelineDocument>(
                &std::fs::read(timeline)?,
            )?;
            let control_plan = build_replay_control_plan(&director, &timeline)?;
            write_json_pretty(&output, &control_plan)?;
            println!("{}", output.canonicalize()?.display());
        }
        Commands::VconsoleProbe { timeout_seconds } => {
            let report = probe_vconsole(Duration::from_secs(timeout_seconds))?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Commands::VconsoleExec {
            commands,
            timeout_seconds,
        } => {
            let report =
                execute_vconsole_commands(&commands, Duration::from_secs(timeout_seconds))?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Commands::Analyze { dem, jobs_dir } => {
            let summary = analyze_dem(&dem, &jobs_dir)?;
            let director_path = Path::new(&summary.job_dir)
                .join("director")
                .join("plan.json");
            println!("{}", director_path.canonicalize()?.display());
        }
        Commands::Doctor => {
            let report = doctor_report();
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }

    Ok(())
}

fn doctor_report() -> DoctorReport {
    DoctorReport {
        ffmpeg: find_command("ffmpeg", &["-version"]),
        ffprobe: find_command("ffprobe", &["-version"]),
        dota2: find_dota2(),
        vconsole2: find_vconsole2(),
    }
}

fn find_command(name: &'static str, args: &[&str]) -> ToolCheck {
    let found = Command::new(name)
        .args(args)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);

    let path = if found { where_command(name) } else { None };
    ToolCheck { name, found, path }
}

fn where_command(name: &str) -> Option<String> {
    let output = Command::new("where.exe").arg(name).output().ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(ToOwned::to_owned)
}

fn find_dota2() -> ToolCheck {
    if let Some(path) = env_path("DOTA2_EXE") {
        return path_check("dota2", path);
    }

    let suffix = Path::new("Program Files (x86)")
        .join("Steam")
        .join("steamapps")
        .join("common")
        .join("dota 2 beta")
        .join("game")
        .join("bin")
        .join("win64")
        .join("dota2.exe");
    find_on_drives("dota2", &suffix)
}

fn find_vconsole2() -> ToolCheck {
    let suffix = Path::new("Program Files (x86)")
        .join("Steam")
        .join("steamapps")
        .join("common")
        .join("dota 2 beta")
        .join("game")
        .join("bin")
        .join("win64")
        .join("vconsole2.exe");
    find_on_drives("vconsole2", &suffix)
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name).map(PathBuf::from)
}

fn path_check(name: &'static str, path: PathBuf) -> ToolCheck {
    ToolCheck {
        name,
        found: path.is_file(),
        path: path.is_file().then(|| path.display().to_string()),
    }
}

fn find_on_drives(name: &'static str, suffix: &Path) -> ToolCheck {
    for letter in b'C'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        let candidate = Path::new(&root).join(suffix);
        if candidate.is_file() {
            return path_check(name, candidate);
        }
    }

    ToolCheck {
        name,
        found: false,
        path: None,
    }
}
