use clap::{Args, Parser, Subcommand};

use crate::comments::{Observation, ReleaseReason};

#[derive(Debug, Parser, Clone)]
#[command(name = "polyresearch")]
#[command(version)]
#[command(about = "Deterministic state middleware for the polyresearch protocol")]
pub struct Cli {
    #[arg(long, global = true)]
    pub repo: Option<String>,

    #[arg(long, global = true)]
    pub github_debug: bool,

    #[arg(long, global = true)]
    pub json: bool,

    #[arg(long, global = true)]
    pub dry_run: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand, Clone)]
pub enum Commands {
    Bootstrap(BootstrapArgs),
    Init(InitArgs),
    Lead(LeadArgs),
    Pace,
    Status(StatusArgs),
    Contribute(ContributeArgs),
    Claim(IssueArgs),
    BatchClaim(BatchClaimArgs),
    Attempt(AttemptArgs),
    Annotate(AnnotateArgs),
    Release(ReleaseArgs),
    Submit(IssueArgs),
    ReviewClaim(PrArgs),
    Review(ReviewArgs),
    Duties,
    Audit,
    Admin(AdminArgs),
    Sync,
    Generate(GenerateArgs),
    PolicyCheck(PrArgs),
    Decide(PrArgs),
    Prune,
}

#[derive(Debug, Args, Clone, Default)]
pub struct InitArgs {
    #[arg(long)]
    pub node: Option<String>,

    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=100))]
    pub capacity: Option<u8>,
}

#[derive(Debug, Args, Clone)]
pub struct BootstrapArgs {
    pub repo_url: String,

    #[arg(long)]
    pub fork: Option<String>,

    #[arg(long)]
    pub goal: Option<String>,

    #[arg(long)]
    pub pause_after_bootstrap: bool,
}

#[derive(Debug, Args, Clone)]
pub struct LeadArgs {
    #[arg(long, help = "Run a single lead-loop iteration and exit")]
    pub once: bool,

    #[arg(long, default_value_t = 60, hide = true)]
    pub sleep_secs: u64,
}

#[derive(Debug, Args, Clone)]
pub struct StatusArgs {
    #[arg(long)]
    pub tui: bool,
}

#[derive(Debug, Args, Clone)]
pub struct ContributeArgs {
    #[arg(help = "Optional repo to clone before starting the contributor loop")]
    pub repo_url: Option<String>,

    #[arg(
        long,
        help = "Manual cap on parallel experiment agents. Example: --max-parallel 1 runs exactly one thesis at a time."
    )]
    pub max_parallel: Option<usize>,

    #[arg(long, help = "Run a single contributor-loop iteration and exit")]
    pub once: bool,

    #[arg(long, default_value_t = 60, hide = true)]
    pub sleep_secs: u64,
}

#[derive(Debug, Args, Clone)]
pub struct IssueArgs {
    pub issue: u64,
}

#[derive(Debug, Args, Clone)]
pub struct BatchClaimArgs {
    #[arg(long)]
    pub count: Option<usize>,
}

#[derive(Debug, Args, Clone)]
pub struct PrArgs {
    pub pr: u64,
}

#[derive(Debug, Args, Clone)]
pub struct AttemptArgs {
    pub issue: u64,

    #[arg(long)]
    pub metric: f64,

    #[arg(long)]
    pub baseline: f64,

    #[arg(long, value_enum)]
    pub observation: Observation,

    #[arg(long)]
    pub summary: String,

    #[arg(long)]
    pub annotations: Option<String>,
}

#[derive(Debug, Args, Clone)]
pub struct AnnotateArgs {
    pub issue: u64,

    #[arg(long)]
    pub text: String,
}

#[derive(Debug, Args, Clone)]
pub struct ReleaseArgs {
    pub issue: u64,

    #[arg(long, value_enum)]
    pub reason: ReleaseReason,
}

#[derive(Debug, Args, Clone)]
pub struct ReviewArgs {
    pub pr: u64,

    #[arg(long)]
    pub metric: f64,

    #[arg(long)]
    pub baseline: f64,

    #[arg(long, value_enum)]
    pub observation: Observation,
}

#[derive(Debug, Args, Clone)]
pub struct GenerateArgs {
    #[arg(long)]
    pub title: String,

    #[arg(long)]
    pub body: String,
}

#[derive(Debug, Args, Clone)]
pub struct AdminArgs {
    #[command(subcommand)]
    pub command: AdminCommands,
}

#[derive(Debug, Subcommand, Clone)]
pub enum AdminCommands {
    ReleaseClaim(AdminReleaseClaimArgs),
    AcknowledgeInvalid(AdminAcknowledgeInvalidArgs),
    ReopenThesis(AdminReopenThesisArgs),
    ReconcileLedger,
}

#[derive(Debug, Args, Clone)]
pub struct AdminReleaseClaimArgs {
    pub issue: u64,

    #[arg(long)]
    pub node: String,

    #[arg(long, value_enum)]
    pub reason: ReleaseReason,

    #[arg(
        long,
        default_value = "Lead repair released the stale or invalid claim."
    )]
    pub note: String,
}

#[derive(Debug, Args, Clone)]
pub struct AdminAcknowledgeInvalidArgs {
    pub comment_id: u64,

    #[arg(long)]
    pub note: String,
}

#[derive(Debug, Args, Clone)]
pub struct AdminReopenThesisArgs {
    pub issue: u64,

    #[arg(long, default_value = "Lead repair reopened the thesis.")]
    pub note: String,
}
