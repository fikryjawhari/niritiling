use crate::connection::{NiriConnection, NiriState, WindowPosition};
use anyhow::Result;
use log::{debug, error, info};
use niri_ipc::{Action, Event, SizeChange, Window};
use std::collections::{HashMap, HashSet};

// A window is considered "at full width" if its tile occupies more than 90% of the output.
const FULL_WIDTH_RATIO_THRESHOLD: f64 = 0.9;

// The proportion niritiling sets a column to when it is the only window (single column).
// In niri-ipc, SizeChange::SetProportion uses 0-100 scale (50.0 = 50%).
const SINGLE_WINDOW_PROPORTION: f64 = 100.0;

// The proportion niritiling resets a column to when a second window appears.
// Should match default-column-width in your niri config (0.49999 proportion = 49.999 here).
const DEFAULT_PROPORTION: f64 = 49.999;

pub struct NiriContext {
    pub connection: Box<dyn NiriConnection>,
    pub tracked_window_positions: HashMap<u64, WindowPosition>,
    pub debounced_maximize_state: HashMap<u64, (bool, std::time::Instant)>,
    /// Window IDs that niritiling itself set to full width via SetColumnWidth.
    /// Used to distinguish niritiling-managed full-width from windows the user
    /// manually maximized or resized — those are never touched by niritiling.
    pub niritiling_full_width: HashSet<u64>,
}

impl NiriContext {
    pub fn new(connection: Box<dyn NiriConnection>) -> Self {
        Self {
            connection,
            tracked_window_positions: HashMap::new(),
            debounced_maximize_state: HashMap::new(),
            niritiling_full_width: HashSet::new(),
        }
    }

    fn send_action(&mut self, action: Action) -> Result<()> {
        self.connection.send_action(action)
    }

    fn query_focused_window(&mut self) -> Result<Option<u64>> {
        self.connection.query_focused_window()
    }

    fn query_full_state(&mut self) -> Result<NiriState> {
        self.connection.query_full_state()
    }

    fn is_full_width(
        &self,
        window_id: u64,
        state: &NiriState,
        windows_map: &HashMap<u64, &Window>,
    ) -> bool {
        if let Some(w) = windows_map.get(&window_id) {
            if let Some(ws_id) = w.workspace_id {
                if let Some(output_name) = state.ws_outputs.get(&ws_id) {
                    if let Some(&output_width) = state.output_widths.get(output_name) {
                        if output_width <= 0.0 {
                            return false;
                        }
                        let tile_width = w.layout.tile_size.0;
                        let ratio = tile_width / output_width;
                        debug!(
                            "window {} tile_width={:.0} output_width={:.0} ratio={:.2}",
                            window_id, tile_width, output_width, ratio
                        );
                        return ratio > FULL_WIDTH_RATIO_THRESHOLD;
                    }
                }
            }
        }
        false
    }

    /// Sets the width of `target_window_id`'s column to `proportion` (0–100 scale).
    /// Temporarily shifts focus to the target window if needed, then restores it
    /// if `restore_focus` is true.
    /// Updates `niritiling_full_width` to track which windows niritiling has set
    /// to full width, so user-initiated maximization is never overridden.
    fn perform_set_width_action(
        &mut self,
        target_window_id: u64,
        proportion: f64,
        restore_focus: bool,
    ) -> Result<()> {
        let original_focus = self.query_focused_window().ok().flatten();

        if original_focus != Some(target_window_id) {
            self.send_action(Action::FocusWindow {
                id: target_window_id,
            })?;
        }

        self.send_action(Action::SetColumnWidth {
            change: SizeChange::SetProportion(proportion),
        })?;

        // Track whether this window is at niritiling-managed full width.
        if (proportion - SINGLE_WINDOW_PROPORTION).abs() < 1e-9 {
            self.niritiling_full_width.insert(target_window_id);
        } else {
            self.niritiling_full_width.remove(&target_window_id);
        }

        if restore_focus {
            if let Some(orig_id) = original_focus {
                if orig_id != target_window_id {
                    debug!("restoring focus to {}", orig_id);
                    let _ = self.send_action(Action::FocusWindow { id: orig_id });
                }
            }
        }
        Ok(())
    }

    pub fn evaluate_workspace(
        &mut self,
        ws_id: u64,
        state: &NiriState,
        windows_map: &HashMap<u64, &Window>,
    ) -> Result<()> {
        let tiled_windows: Vec<&Window> = state
            .windows
            .iter()
            .filter(|w| w.workspace_id == Some(ws_id) && !w.is_floating)
            .collect();

        if tiled_windows.is_empty() {
            return Ok(());
        }

        let mut unique_columns = std::collections::HashSet::new();
        for w in &tiled_windows {
            if let Some((col_idx, _)) = w.layout.pos_in_scrolling_layout {
                unique_columns.insert(col_idx);
            }
        }

        let column_count = unique_columns.len();

        if column_count == 0 {
            return Ok(());
        } else if column_count == 1 {
            let win_id = tiled_windows[0].id;
            if !self.is_full_width(win_id, state, windows_map) {
                let now = std::time::Instant::now();
                if let Some(&(target_full_width, last_time)) =
                    self.debounced_maximize_state.get(&win_id)
                {
                    if target_full_width
                        && now.duration_since(last_time) < std::time::Duration::from_millis(200)
                    {
                        debug!(
                            "workspace {}: skipping full-width for window {} due to debounce",
                            ws_id, win_id
                        );
                        return Ok(());
                    }
                }
                self.debounced_maximize_state.insert(win_id, (true, now));

                info!(
                    "workspace {}: single column -> setting window {} to full width",
                    ws_id, win_id
                );
                self.perform_set_width_action(win_id, SINGLE_WINDOW_PROPORTION, true)?;
            }
        } else {
            let mut cols_vec: Vec<usize> = unique_columns.into_iter().collect();
            cols_vec.sort_unstable();

            let mut did_reset_width = false;
            for &col_idx in &cols_vec {
                if let Some(w) = tiled_windows
                    .iter()
                    .find(|w| w.layout.pos_in_scrolling_layout.map(|(c, _)| c) == Some(col_idx))
                {
                    // Only reset if niritiling was the one that set this window to full
                    // width. If the user manually maximized or resized it, we leave it alone.
                    if self.is_full_width(w.id, state, windows_map)
                        && self.niritiling_full_width.contains(&w.id)
                    {
                        let now = std::time::Instant::now();
                        if let Some(&(target_full_width, last_time)) =
                            self.debounced_maximize_state.get(&w.id)
                        {
                            if !target_full_width
                                && now.duration_since(last_time)
                                    < std::time::Duration::from_millis(200)
                            {
                                debug!(
                                    "workspace {}: skipping width reset for window {} due to debounce",
                                    ws_id, w.id
                                );
                                continue;
                            }
                        }
                        self.debounced_maximize_state.insert(w.id, (false, now));

                        info!(
                            "workspace {}: multiple columns -> resetting width of window {} in column {}",
                            ws_id, w.id, col_idx
                        );
                        self.perform_set_width_action(w.id, DEFAULT_PROPORTION, true)?;
                        did_reset_width = true;
                    }
                }
            }

            if did_reset_width {
                // Wait for niri to settle the layout before adjusting the viewport,
                // otherwise on-overflow centering may fire against the old (full-width) layout.
                std::thread::sleep(std::time::Duration::from_millis(100));
                let _ = self.send_action(Action::FocusColumnLeft {});
                let _ = self.send_action(Action::FocusColumnRight {});
            }
        }
        Ok(())
    }

    pub fn handle_event(&mut self, event: Event) -> Result<()> {
        let mut affected_workspaces = Vec::new();
        let mut closed_positions: Vec<WindowPosition> = Vec::new();

        match event {
            Event::WindowsChanged { windows } => {
                debug!("full windows change event received");
                let mut new_tracked = HashMap::with_capacity(windows.len());

                for w in windows {
                    if !w.is_floating {
                        if let Some(ws_id) = w.workspace_id {
                            let (col, tile) = w
                                .layout
                                .pos_in_scrolling_layout
                                .map(|(c, t)| (Some(c), Some(t)))
                                .unwrap_or((None, None));

                            let pos = WindowPosition {
                                workspace_id: ws_id,
                                column: col,
                                tile,
                            };
                            new_tracked.insert(w.id, pos);
                        }
                    }
                }

                for (&id, &pos) in &new_tracked {
                    if self.tracked_window_positions.get(&id) != Some(&pos) {
                        affected_workspaces.push(pos.workspace_id);
                    }
                }
                for (&id, &pos) in &self.tracked_window_positions {
                    if new_tracked.get(&id) != Some(&pos) {
                        affected_workspaces.push(pos.workspace_id);
                    }
                }

                self.tracked_window_positions = new_tracked;
            }

            Event::WindowOpenedOrChanged { window } => {
                let id = window.id;
                let ws_id_opt = window.workspace_id;
                let is_floating = window.is_floating;

                let old_pos = self.tracked_window_positions.get(&id).copied();

                if is_floating {
                    if let Some(pos) = old_pos {
                        self.tracked_window_positions.remove(&id);
                        self.niritiling_full_width.remove(&id);
                        info!(
                            "window {} became floating, re-evaluating ws {}",
                            id, pos.workspace_id
                        );
                        affected_workspaces.push(pos.workspace_id);
                    }
                } else if let Some(ws_id) = ws_id_opt {
                    let (col, tile) = window
                        .layout
                        .pos_in_scrolling_layout
                        .map(|(c, t)| (Some(c), Some(t)))
                        .unwrap_or((None, None));

                    let new_pos = WindowPosition {
                        workspace_id: ws_id,
                        column: col,
                        tile,
                    };

                    self.tracked_window_positions.insert(id, new_pos);
                    debug!(
                        "window {} position updated to {:?}, re-evaluating",
                        id, new_pos
                    );
                    affected_workspaces.push(ws_id);
                    if let Some(old) = old_pos {
                        if old.workspace_id != ws_id {
                            affected_workspaces.push(old.workspace_id);
                        }
                    }
                }
            }

            Event::WindowLayoutsChanged { changes } => {
                for (id, layout) in changes {
                    if let Some(pos) = self.tracked_window_positions.get_mut(&id) {
                        let (col, tile) = layout
                            .pos_in_scrolling_layout
                            .map(|(c, t)| (Some(c), Some(t)))
                            .unwrap_or((None, None));

                        debug!(
                            "window {} layout updated to column {:?}, tile {:?}, re-evaluating ws {}",
                            id, col, tile, pos.workspace_id
                        );
                        pos.column = col;
                        pos.tile = tile;
                        affected_workspaces.push(pos.workspace_id);
                    }
                }
            }

            Event::WindowClosed { id } => {
                if let Some(pos) = self.tracked_window_positions.remove(&id) {
                    self.niritiling_full_width.remove(&id);
                    info!(
                        "window {} closed, re-evaluating ws {}",
                        id, pos.workspace_id
                    );
                    affected_workspaces.push(pos.workspace_id);
                    closed_positions.push(pos);
                }
            }

            _ => {}
        }

        if !affected_workspaces.is_empty() {
            affected_workspaces.sort_unstable();
            affected_workspaces.dedup();

            std::thread::sleep(std::time::Duration::from_millis(20));

            let state = self.query_full_state()?;
            let windows_map: HashMap<u64, &Window> =
                state.windows.iter().map(|w| (w.id, w)).collect();

            for ws_id in affected_workspaces {
                if let Err(e) = self.evaluate_workspace(ws_id, &state, &windows_map) {
                    error!("error evaluating workspace {}: {:?}", ws_id, e);
                }
            }

            for closed_pos in &closed_positions {
                // If 2+ columns remain after the close, snap the viewport so the
                // last column is right-edge-aligned, eliminating empty space on the right.
                // FocusColumnFirst then FocusColumnLast achieves this regardless of widths.
                // Focus is not restored — niri already moved it to the next sensible window.
                let remaining_col_count = self
                    .tracked_window_positions
                    .values()
                    .filter(|p| p.workspace_id == closed_pos.workspace_id)
                    .filter_map(|p| p.column)
                    .collect::<HashSet<_>>()
                    .len();

                if remaining_col_count >= 2 {
                    debug!(
                        "closed window on ws {}, {} columns remain — snapping viewport to last column",
                        closed_pos.workspace_id, remaining_col_count
                    );
                    let _ = self.send_action(Action::FocusColumnFirst {});
                    let _ = self.send_action(Action::FocusColumnLast {});
                }
            }
        }

        Ok(())
    }
}
