use color_eyre::eyre::{Result, eyre};

use crate::commands::AppContext;
use crate::comments::ReleaseReason;
use crate::ledger::Ledger;
use crate::state::{PullRequestState, RepositoryState, ThesisPhase, ThesisState};

pub fn ensure_lead(ctx: &AppContext) -> Result<String> {
    let login = ctx.github.current_login()?;
    let expected = ctx.config.lead_login()?;
    if login != expected {
        return Err(eyre!(
            "this command is lead-only; current GitHub login is `{login}`, but `lead_github_login` is `{expected}`"
        ));
    }
    Ok(login)
}

pub fn find_thesis<'a>(
    repo_state: &'a RepositoryState,
    issue_number: u64,
) -> Result<&'a ThesisState> {
    repo_state
        .get_thesis(issue_number)
        .ok_or_else(|| eyre!("thesis #{} not found", issue_number))
}

pub fn require_claimable_thesis<'a>(
    repo_state: &'a RepositoryState,
    issue_number: u64,
) -> Result<&'a ThesisState> {
    let thesis = find_thesis(repo_state, issue_number)?;
    if thesis.issue.state != "OPEN" {
        return Err(eyre!("thesis #{} is not open", issue_number));
    }
    if !thesis.approved {
        return Err(eyre!("thesis #{} is not approved yet", issue_number));
    }
    if !matches!(thesis.phase, ThesisPhase::Approved) {
        return Err(eyre!(
            "thesis #{} is not claimable; current state is {:?}",
            issue_number,
            thesis.phase
        ));
    }
    Ok(thesis)
}

pub fn require_claimed_thesis<'a>(
    repo_state: &'a RepositoryState,
    issue_number: u64,
    node: &str,
) -> Result<&'a ThesisState> {
    let thesis = find_thesis(repo_state, issue_number)?;
    if thesis.issue.state != "OPEN" {
        return Err(eyre!("thesis #{} is closed", issue_number));
    }
    if !thesis.is_claimed_by(node) {
        return Err(eyre!(
            "thesis #{} is not currently claimed by node `{}`",
            issue_number,
            node
        ));
    }
    Ok(thesis)
}

pub fn require_pull_request<'a>(
    repo_state: &'a RepositoryState,
    pr_number: u64,
) -> Result<(&'a ThesisState, &'a PullRequestState)> {
    repo_state
        .get_pull_request(pr_number)
        .ok_or_else(|| eyre!("PR #{} not found", pr_number))
}

pub fn require_reviewable_pr<'a>(
    repo_state: &'a RepositoryState,
    pr_number: u64,
    reviewer_login: &str,
) -> Result<(&'a ThesisState, &'a PullRequestState)> {
    let (thesis, pr_state) = require_pull_request(repo_state, pr_number)?;
    if !pr_state.policy_pass {
        return Err(eyre!("PR #{} has not passed policy check yet", pr_number));
    }
    if pr_state.decision.is_some() {
        return Err(eyre!("PR #{} already has a decision", pr_number));
    }
    if pr_state
        .pr
        .author
        .as_ref()
        .map(|author| author.login == reviewer_login)
        .unwrap_or(false)
    {
        return Err(eyre!("you cannot review your own PR"));
    }
    Ok((thesis, pr_state))
}

pub fn require_claimed_review_pr<'a>(
    repo_state: &'a RepositoryState,
    pr_number: u64,
    node: &str,
) -> Result<(&'a ThesisState, &'a PullRequestState)> {
    let (thesis, pr_state) = require_pull_request(repo_state, pr_number)?;
    if !pr_state
        .review_claims
        .iter()
        .any(|claim| claim.node == node)
    {
        return Err(eyre!(
            "PR #{} has not been review-claimed by node `{}`",
            pr_number,
            node
        ));
    }
    Ok((thesis, pr_state))
}

pub fn require_decidable_pr<'a>(
    repo_state: &'a RepositoryState,
    pr_number: u64,
) -> Result<(&'a ThesisState, &'a PullRequestState)> {
    let (thesis, pr_state) = require_pull_request(repo_state, pr_number)?;
    if pr_state.decision.is_some() {
        return Err(eyre!("PR #{} already has a decision", pr_number));
    }
    if !pr_state.policy_pass {
        return Err(eyre!("PR #{} has not passed policy-check yet", pr_number));
    }
    Ok((thesis, pr_state))
}

pub fn ensure_clean_audit(repo_state: &RepositoryState, action: &str) -> Result<()> {
    if !repo_state.audit_findings.is_empty() {
        return Err(eyre!(
            "cannot {} while audit findings are present; run `polyresearch audit` and resolve them through `polyresearch admin ...` first",
            action
        ));
    }
    Ok(())
}

pub fn ensure_lead_ready(
    ctx: &AppContext,
    repo_state: &RepositoryState,
) -> Result<(String, Ledger)> {
    let login = ensure_lead(ctx)?;
    ensure_clean_audit(repo_state, "proceed")?;
    let ledger = Ledger::load(&ctx.repo_root)?;
    if !ledger.is_current(repo_state) {
        return Err(eyre!(
            "results.tsv is stale; run `polyresearch sync` before proceeding"
        ));
    }
    Ok((login, ledger))
}

/// After a release with `no_improvement`, re-derive state and close the issue
/// if the thesis has become Exhausted (no remaining active claims).
pub async fn close_if_exhausted(
    ctx: &AppContext,
    issue: u64,
    reason: ReleaseReason,
) -> Result<()> {
    if reason != ReleaseReason::NoImprovement {
        return Ok(());
    }
    let updated = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    if let Some(t) = updated.theses.iter().find(|t| t.issue.number == issue) {
        if matches!(t.phase, ThesisPhase::Exhausted) {
            ctx.github.close_issue(issue)?;
        }
    }
    Ok(())
}
