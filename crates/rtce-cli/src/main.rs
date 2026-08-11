use clap::{Args, Parser, Subcommand, ValueEnum};
use rtce_runner::SimulationMode;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const DEMO_GAME: &str = include_str!("../../rtce/tests/fixtures/guide/07-gamedef.json");
const DEMO_BUILD: &str = include_str!("../../rtce/tests/fixtures/guide/07-build.json");
const DEMO_SCENARIO: &str = include_str!("../../rtce/tests/fixtures/guide/07-scenario.json");
const DEMO_SIMDEF: &str = include_str!("../../rtce/tests/fixtures/guide/07-simdef.json");
const DEMO_ROTATION: &str = include_str!("../../rtce/tests/fixtures/guide/07-rotation.json");

#[derive(Debug, Parser)]
#[command(
    name = "rtce",
    version,
    about = "Run config-driven RPG stat-sheet calculations and timeline simulations"
)]
struct Cli {
    /// Emit one-line JSON instead of indented JSON.
    #[arg(long, global = true)]
    compact: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Compile a GameDef and calculate named objectives.
    Evaluate(CalcFiles),
    /// Calculate objectives with phase, stage, and event-branch traces.
    Explain(CalcFiles),
    /// Run a priority-list timeline in expected-value or Monte Carlo mode.
    Simulate(SimFiles),
    /// Run a bundled tutorial configuration without preparing any files.
    Demo {
        #[arg(value_enum, default_value_t = DemoKind::Calc)]
        kind: DemoKind,
    },
}

#[derive(Debug, Clone, Args)]
struct CalcFiles {
    /// Game algorithm JSON.
    #[arg(long, value_name = "FILE")]
    game: PathBuf,
    /// Character/build JSON.
    #[arg(long, value_name = "FILE")]
    build: PathBuf,
    /// Fight/scenario JSON.
    #[arg(long, value_name = "FILE")]
    scenario: PathBuf,
}

#[derive(Debug, Clone, Args)]
struct SimFiles {
    #[command(flatten)]
    calc: CalcFiles,
    /// Resources, actions, buffs, and procs JSON.
    #[arg(long, value_name = "FILE")]
    sim: PathBuf,
    /// Priority-list rotation JSON.
    #[arg(long, value_name = "FILE")]
    rotation: PathBuf,
    /// Simulation fidelity.
    #[arg(long, value_enum, default_value_t = SimModeArg::Expected)]
    mode: SimModeArg,
    /// Number of timelines sampled in Monte Carlo mode.
    #[arg(long, default_value_t = 1000)]
    iterations: u32,
    /// Reproducible Monte Carlo master seed.
    #[arg(long, default_value_t = 42)]
    seed: u64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SimModeArg {
    Expected,
    MonteCarlo,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DemoKind {
    Calc,
    Sim,
    MonteCarlo,
}

fn read(path: &Path, kind: &str) -> Result<String, String> {
    fs::read_to_string(path)
        .map_err(|error| format!("could not read {kind} `{}`: {error}", path.display()))
}

fn read_calc(files: &CalcFiles) -> Result<(String, String, String), String> {
    Ok((
        read(&files.game, "gamedef")?,
        read(&files.build, "build")?,
        read(&files.scenario, "scenario")?,
    ))
}

fn run(cli: &Cli) -> Result<Value, String> {
    match &cli.command {
        Command::Evaluate(files) => {
            let (game, build, scenario) = read_calc(files)?;
            rtce_runner::evaluate(&game, &build, &scenario).map_err(|error| error.to_string())
        }
        Command::Explain(files) => {
            let (game, build, scenario) = read_calc(files)?;
            rtce_runner::explain(&game, &build, &scenario).map_err(|error| error.to_string())
        }
        Command::Simulate(files) => {
            let (game, build, scenario) = read_calc(&files.calc)?;
            let sim = read(&files.sim, "simdef")?;
            let rotation = read(&files.rotation, "rotation")?;
            let mode = match files.mode {
                SimModeArg::Expected => SimulationMode::Expected,
                SimModeArg::MonteCarlo => SimulationMode::MonteCarlo {
                    iterations: files.iterations,
                    seed: files.seed,
                },
            };
            rtce_runner::simulate(&game, &build, &scenario, &sim, &rotation, mode)
                .map_err(|error| error.to_string())
        }
        Command::Demo { kind } => match kind {
            DemoKind::Calc => rtce_runner::evaluate(DEMO_GAME, DEMO_BUILD, DEMO_SCENARIO),
            DemoKind::Sim => rtce_runner::simulate(
                DEMO_GAME,
                DEMO_BUILD,
                DEMO_SCENARIO,
                DEMO_SIMDEF,
                DEMO_ROTATION,
                SimulationMode::Expected,
            ),
            DemoKind::MonteCarlo => rtce_runner::simulate(
                DEMO_GAME,
                DEMO_BUILD,
                DEMO_SCENARIO,
                DEMO_SIMDEF,
                DEMO_ROTATION,
                SimulationMode::MonteCarlo {
                    iterations: 1000,
                    seed: 7,
                },
            ),
        }
        .map_err(|error| error.to_string()),
    }
}

fn main() {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(value) => {
            let rendered = if cli.compact {
                serde_json::to_string(&value)
            } else {
                serde_json::to_string_pretty(&value)
            }
            .expect("runner output is JSON serializable");
            println!("{rendered}");
        }
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}
