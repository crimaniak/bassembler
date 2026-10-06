mod assemble;
mod config;
mod git;
mod order;
mod preflight;
mod resolver;
mod temp;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use assemble::{ConflictMode, Options};
use config::{IssuesSpec, Overrides};

const AFTER_HELP: &str = "Config file (bassembler.json):
  {
    \"base\": \"v1.4.0\",                      branch or tag to start from
    \"target\": \"release-1.4-hotfix\",        branch to create
    \"projects\": [\"../backend\", \"../web\"],  git working trees, relative to the config file
    \"issues\": [\"PRJ-101\", \"PRJ-117\"],      codes, or the name of a file with them
    \"resolver\": \"fix.bat\",                 optional, same as --resolver
    \"conflict\": \"skip\"                     optional, abort (default) or skip
  }

Examples:
  bassembler --config bassembler.json --dry-run
  bassembler --base v1.4.0 --target hotfix --project ../api,../web --issues PRJ-1,PRJ-2

Exit codes: 0 success, 2 finished but some issues were skipped, 1 error or aborted.";

/// Assembles a Git branch from issue-related commits, as described by a bassembler.json file.
///
/// In every project listed it creates the target branch at the base (a branch or tag), finds the
/// commits whose first message line mentions one of the issue codes, and cherry-picks them issue
/// by issue, in history order. Conflicts can be handed to a resolver program, and an issue that
/// still conflicts is either skipped in all projects or stops the run.
///
/// Settings come from the config file; any of --base, --target, --project and --issues given on
/// the command line overrides the file's value. Run with no arguments to see this help.
#[derive(Parser)]
#[command(version, bin_name = "bassembler", after_long_help = AFTER_HELP)]
struct Cli {
    /// Path to the config file (default: bassembler.json in the current directory, if it exists).
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Branch or tag the new branch starts from. Overrides 'base' from the config file.
    #[arg(long)]
    base: Option<String>,

    /// Branch to create. Overrides 'target' from the config file.
    #[arg(long)]
    target: Option<String>,

    /// Project directories, comma-separated or repeated; relative to the current directory.
    /// Overrides 'projects' from the config file.
    #[arg(
        long = "project",
        visible_alias = "projects",
        value_name = "DIR",
        value_delimiter = ','
    )]
    projects: Vec<String>,

    /// Issue codes, comma-separated or repeated, or the name of a file listing them (JSON array
    /// or one per line). Overrides 'issues' from the config file.
    #[arg(
        long = "issues",
        visible_alias = "issue",
        value_name = "ISSUES",
        value_delimiter = ','
    )]
    issues: Vec<String>,

    /// If the target branch exists, rename it to <target>_backup_<timestamp> (when the new branch
    /// is ready) instead of failing.
    #[arg(long = "override")]
    override_target: bool,

    /// Program or batch file run in the project directory on a cherry-pick conflict.
    /// It must exit with 0 if it resolved the conflict, 1 otherwise. On Windows a .ps1 script is
    /// run through PowerShell. Overrides 'resolver' from the config file.
    #[arg(long, value_name = "UTILITY")]
    resolver: Option<PathBuf>,

    /// What to do with an unresolved conflict (default: abort). Overrides 'conflict' from the
    /// config file.
    #[arg(long, value_enum)]
    conflict: Option<ConflictMode>,

    /// Only check the projects and print the commits that would be picked.
    #[arg(long)]
    dry_run: bool,
}

fn main() -> ExitCode {
    // Without arguments there is nothing to act on by accident: show the help instead.
    if std::env::args_os().len() <= 1 {
        // Ignore write errors (e.g. a closed pipe): there is nothing useful to do about them.
        let _ = Cli::command().print_long_help();
        return ExitCode::SUCCESS;
    }
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code as u8),
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<i32> {
    // A single value naming an existing file is an issues file, anything else is a list of codes.
    let issues = match cli.issues.as_slice() {
        [] => None,
        [one] if std::path::Path::new(one).is_file() => Some(IssuesSpec::File(one.clone())),
        _ => Some(IssuesSpec::List(cli.issues)),
    };
    let overrides = Overrides {
        base: cli.base,
        target: cli.target,
        projects: (!cli.projects.is_empty()).then_some(cli.projects),
        issues,
        resolver: cli.resolver,
        conflict: cli.conflict,
    };
    let cfg = config::load(cli.config.as_deref(), overrides)?;

    // The resolver runs in each project directory, so a relative path must be anchored here.
    let resolver = cfg.resolver.clone().map(|r| {
        if r.components().count() > 1 || r.exists() {
            std::path::absolute(&r).unwrap_or(r)
        } else {
            r
        }
    });

    let opts = Options {
        override_target: cli.override_target,
        resolver,
        conflict: cfg.conflict,
        dry_run: cli.dry_run,
    };
    assemble::run(&cfg, &opts)
}
