use ratatui::widgets::TableState;

use crate::commands::AppContext;
use crate::ledger::Ledger;
use crate::state::RepositoryState;

pub struct DashboardApp {
    pub repo_state: RepositoryState,
    pub ledger: Ledger,
    pub table_state: TableState,
    pub show_details: bool,
}

impl DashboardApp {
    pub fn new(repo_state: RepositoryState, ledger: Ledger) -> Self {
        let mut table_state = TableState::default();
        if !repo_state.theses.is_empty() {
            table_state.select(Some(0));
        }

        Self {
            repo_state,
            ledger,
            table_state,
            show_details: false,
        }
    }

    pub fn next(&mut self) {
        if self.repo_state.theses.is_empty() {
            self.table_state.select(None);
            return;
        }

        let next = match self.table_state.selected() {
            Some(index) if index + 1 < self.repo_state.theses.len() => index + 1,
            _ => 0,
        };
        self.table_state.select(Some(next));
    }

    pub fn previous(&mut self) {
        if self.repo_state.theses.is_empty() {
            self.table_state.select(None);
            return;
        }

        let previous = match self.table_state.selected() {
            Some(0) | None => self.repo_state.theses.len() - 1,
            Some(index) => index - 1,
        };
        self.table_state.select(Some(previous));
    }

    pub fn toggle_detail(&mut self) {
        self.show_details = !self.show_details;
    }

    pub fn refresh(&mut self, ctx: &AppContext) -> color_eyre::eyre::Result<()> {
        self.repo_state = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(RepositoryState::derive(&ctx.github, &ctx.config))
        })?;
        self.ledger = Ledger::load(&ctx.repo_root)?;
        if self.repo_state.theses.is_empty() {
            self.table_state.select(None);
        } else if self.table_state.selected().is_none() {
            self.table_state.select(Some(0));
        }
        Ok(())
    }

    pub fn selected_thesis(&self) -> Option<&crate::state::ThesisState> {
        self.table_state
            .selected()
            .and_then(|index| self.repo_state.theses.get(index))
    }
}
