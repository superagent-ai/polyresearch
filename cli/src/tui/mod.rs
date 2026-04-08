pub mod app;
pub mod views;

use std::io::{self, Stdout};
use std::time::Duration;

use color_eyre::eyre::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::commands::AppContext;
use crate::ledger::Ledger;
use crate::state::RepositoryState;

pub fn run_dashboard(ctx: &AppContext, repo_state: RepositoryState, ledger: Ledger) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_event_loop(
        ctx,
        &mut terminal,
        app::DashboardApp::new(repo_state, ledger),
    );

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_event_loop(
    ctx: &AppContext,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mut app: app::DashboardApp,
) -> Result<()> {
    loop {
        terminal.draw(|frame| views::draw(frame, &app, ctx))?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Down => app.next(),
                    KeyCode::Up => app.previous(),
                    KeyCode::Enter => app.toggle_detail(),
                    KeyCode::Char('r') => app.refresh(ctx)?,
                    _ => {}
                }
            }
        }
    }

    Ok(())
}
