//! Text ⇄ binary for plans and runs, plus the checker that refuses a plan before it runs.
//!
//! The binary form is what a Rust consumer loads; the textual form is what a human writes,
//! reviews and stores. `convert` moves between them without touching the model.

use anyhow::{bail, Context, Result};
use schematic::{
    decode_plan, decode_run, encode_plan, encode_run, plan_from_bytes, run_from_bytes,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("schematic: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<ExitCode> {
    let Some(command) = args.first().map(String::as_str) else {
        usage();
        return Ok(ExitCode::FAILURE);
    };
    match command {
        "check" => check(&args[1..]),
        "convert" => convert(&args[1..]),
        "ready" => readiness(&args[1..]),
        "-h" | "--help" | "help" => {
            usage();
            Ok(ExitCode::SUCCESS)
        }
        other => bail!("unknown command {other:?} (try --help)"),
    }
}

fn usage() {
    println!(
        "schematic — validate and convert plans

USAGE:
    schematic check <plan>                    refuse a plan before it runs
    schematic convert --to bin|json <in> [out]
    schematic ready <plan> <run>              what may run now

A plan or a run may be given in either form; the leading bytes decide."
    );
}

fn check(args: &[String]) -> Result<ExitCode> {
    let path = one(args, "check <plan>")?;
    let plan = read_plan(&path)?;
    let problems = schematic::validate(&plan);
    if problems.is_empty() {
        println!("ok: {} steps", plan.steps.len());
        return Ok(ExitCode::SUCCESS);
    }
    for problem in &problems {
        println!("- {problem}");
    }
    Ok(ExitCode::FAILURE)
}

fn convert(args: &[String]) -> Result<ExitCode> {
    let mut to = None;
    let mut files: Vec<PathBuf> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--to" {
            to = args.get(index + 1).cloned();
            index += 2;
            continue;
        }
        files.push(PathBuf::from(&args[index]));
        index += 1;
    }
    let Some(target) = to.as_deref() else {
        bail!("convert needs --to bin|json");
    };
    let Some(input) = files.first() else {
        bail!("convert needs an input path");
    };
    let bytes = std::fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let binary = bytes.starts_with(b"SCHM");

    let output: Vec<u8> = match (target, binary) {
        ("bin", true) | ("json", false) => bytes,
        ("bin", false) => {
            let text = std::str::from_utf8(&bytes).context("text form is not UTF-8")?;
            match schematic::plan_from_json(text) {
                Ok(plan) => encode_plan(&plan),
                Err(_) => {
                    let run = schematic::run_from_json(text)
                        .context("document is neither a plan nor a run")?;
                    encode_run(&run)
                }
            }
        }
        ("json", true) => match decode_plan(&bytes) {
            Ok(plan) => schematic::plan_to_json(&plan).into_bytes(),
            Err(_) => {
                let run = decode_run(&bytes).context("document is not a plan nor a run")?;
                schematic::run_to_json(&run).into_bytes()
            }
        },
        (other, _) => bail!("--to must be bin or json, not {other:?}"),
    };

    match files.get(1) {
        Some(path) => {
            std::fs::write(path, &output).with_context(|| format!("writing {}", path.display()))?;
            println!("{} bytes -> {}", output.len(), path.display());
        }
        None => {
            use std::io::Write;
            std::io::stdout().write_all(&output)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn readiness(args: &[String]) -> Result<ExitCode> {
    if args.len() != 2 {
        bail!("usage: schematic ready <plan> <run>");
    }
    let plan = read_plan(Path::new(&args[0]))?;
    let bytes = std::fs::read(&args[1]).with_context(|| format!("reading {}", args[1]))?;
    let run = run_from_bytes(&bytes).map_err(|err| anyhow::anyhow!("{err}"))?;
    let answer = schematic::ready(&plan, &run);

    let rendered = serde_json::json!({
        "ready": answer.ready.iter().map(|a| serde_json::json!({
            "step": a.step,
            "pass": a.pass,
            "attempt": a.attempt,
            "inputs": a.inputs,
        })).collect::<Vec<_>>(),
        "blocked": answer.blocked.iter().map(|b| serde_json::json!({
            "step": b.step,
            "pass": b.pass,
            "missing": b.missing,
        })).collect::<Vec<_>>(),
        "complete": answer.complete,
        "stalled": answer.stalled,
    });
    println!("{}", serde_json::to_string_pretty(&rendered)?);
    Ok(if answer.stalled {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn read_plan(path: &Path) -> Result<schematic::Plan> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    plan_from_bytes(&bytes).map_err(|err| anyhow::anyhow!("{err}"))
}

fn one(args: &[String], usage: &str) -> Result<PathBuf> {
    match args {
        [path] => Ok(PathBuf::from(path)),
        _ => bail!("usage: schematic {usage}"),
    }
}
