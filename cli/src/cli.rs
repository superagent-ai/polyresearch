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
    pub json: bool,

    #[arg(long, global = true)]
    pub dry_run: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand, Clone)]
pub enum Commands {
    Init(InitArgs),
    Pace,
    Status(StatusArgs),
    Claim(IssueArgs),
    Attempt(AttemptArgs),
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

#[derive(Debug, Args, Clone)]
pub struct InitArgs {
    #[arg(long)]
    pub node: Option<String>,

    #[arg(long)]
    pub resource_policy: Option<String>,
}

#[derive(Debug, Args, Clone)]
pub struct StatusArgs {
    #[arg(long)]
    pub tui: bool,
}

#[derive(Debug, Args, Clone)]
pub struct IssueArgs {
    pub issue: u64,

    #[arg(long)]
    pub no_worktree: bool,
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
