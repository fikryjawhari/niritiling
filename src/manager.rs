use crate::connection::{NiriConnection, NiriState, WindowPosition};
use anyhow::Result;
use log::{debug, error, info};
use niri_ipc::{Action, Event, SizeChange, Window};
use std::collections::HashMap;

const MAXIMIZED_RATIO_THRESHOLD: f64 = 0.9;

pub struct NiriContext {
    pub connection: Box<dyn NiriConnection>,
    pub tracked_window_positions: HashMap<u64, WindowPosition>,
    pub debounced_maximize_state: HashMap<u64, (bool, std::time::Instant)>,
    pub tracked_column_widths: HashMap<u64, HashMap<usize, f64>>,
    pub last_maximize_action_time: HashMap<u64, std::time::Instant>,
    pub resize_columns: bool,
}

impl NiriContext {
    pub fn new(connection: Box<dyn NiriConnection>, resize_columns: bool) -> Self {
        Self {
            connection,
            tracked_window_positions: HashMap::new(),
            debounced_maximize_state: HashMap::new(),
            tracked_column_widths: HashMap::new(),
            last_maximize_action_time: HashMap::new(),
            resize_columns,
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

    fn is_maximized(
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
                        return ratio > MAXIMIZED_RATIO_THRESHOLD;
                    }
                }
            }
        }
        false
    }

    fn perform_maximize_action(
        &mut self,
        target_window_id: u64,
        restore_focus: bool,
    ) -> Result<()> {
        let original_focus = self.query_focused_window().ok().flatten();

        if original_focus != Some(target_window_id) {
            self.send_action(Action::FocusWindow {
                id: target_window_id,
            })?;
        }

        self.send_action(Action::MaximizeColumn {})?;

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
            if !self.is_maximized(win_id, state, windows_map) {
                let now = std::time::Instant::now();
                if let Some(&(target_maximized, last_time)) =
                    self.debounced_maximize_state.get(&win_id)
                {
                    if target_maximized
                        && now.duration_since(last_time) < std::time::Duration::from_millis(200)
                    {
                        debug!(
                            "workspace {}: skipping maximize for window {} due to debounce",
                            ws_id, win_id
                        );
                        return Ok(());
                    }
                }
                self.debounced_maximize_state.insert(win_id, (true, now));

                info!(
                    "workspace {}: single column -> maximizing window {}",
                    ws_id, win_id
                );
                self.perform_maximize_action(win_id, true)?;
                self.last_maximize_action_time
                    .insert(ws_id, std::time::Instant::now());
            }
        } else {
            let target_nudge_focus = self.query_focused_window().ok().flatten();
            let mut cols_vec: Vec<usize> = unique_columns.into_iter().collect();
            cols_vec.sort_unstable();

            let mut did_unmaximize = false;
            for &col_idx in &cols_vec {
                if let Some(w) = tiled_windows
                    .iter()
                    .find(|w| w.layout.pos_in_scrolling_layout.map(|(c, _)| c) == Some(col_idx))
                {
                    if self.is_maximized(w.id, state, windows_map) {
                        let now = std::time::Instant::now();
                        if let Some(&(target_maximized, last_time)) =
                            self.debounced_maximize_state.get(&w.id)
                        {
                            if !target_maximized
                                && now.duration_since(last_time)
                                    < std::time::Duration::from_millis(200)
                            {
                                debug!(
                                    "workspace {}: skipping un-maximize for window {} due to debounce",
                                    ws_id, w.id
                                );
                                continue;
                            }
                        }
                        self.debounced_maximize_state.insert(w.id, (false, now));

                        info!(
                            "workspace {}: multiple columns -> un-maximizing window {} in column {}",
                            ws_id, w.id, col_idx
                        );
                        self.perform_maximize_action(w.id, false)?;
                        did_unmaximize = true;
                    }
                }
            }

            if did_unmaximize {
                self.last_maximize_action_time
                    .insert(ws_id, std::time::Instant::now());
                debug!(
                    "workspace {}: waiting for layout to settle before viewport nudge",
                    ws_id
                );
                std::thread::sleep(std::time::Duration::from_millis(50));

                debug!(
                    "workspace {}: nudging viewport left (target focus: {:?})",
                    ws_id, target_nudge_focus
                );
                self.send_action(Action::FocusColumnLeft {})?;
                if let Some(orig_id) = target_nudge_focus {
                    debug!("workspace {}: restoring focus to {}", ws_id, orig_id);
                    let _ = self.send_action(Action::FocusWindow { id: orig_id });
                }
            }
        }
        Ok(())
    }

    fn redistribute_on_column_resize(
        &mut self,
        ws_id: u64,
        state: &NiriState,
        _windows_map: &HashMap<u64, &Window>,
    ) -> Result<()> {
        let output_name = match state.ws_outputs.get(&ws_id) {
            Some(name) => name,
            None => return Ok(()),
        };
        let output_width = match state.output_widths.get(output_name) {
            Some(&w) if w > 0.0 => w,
            _ => return Ok(()),
        };

        let tiled: Vec<&Window> = state
            .windows
            .iter()
            .filter(|w| w.workspace_id == Some(ws_id) && !w.is_floating)
            .filter(|w| w.layout.pos_in_scrolling_layout.is_some())
            .collect();

        let mut current_columns: HashMap<usize, (f64, u64)> = HashMap::new();
        for w in &tiled {
            if let Some((col_idx, _)) = w.layout.pos_in_scrolling_layout {
                current_columns
                    .entry(col_idx)
                    .or_insert((w.layout.tile_size.0, w.id));
            }
        }

        let col_count = current_columns.len();

        // Only handle exactly 2 columns
        if col_count != 2 {
            self.tracked_column_widths.clear();
            return Ok(());
        }

        let prev_widths = self.tracked_column_widths.get(&ws_id);

        let suppressed = self
            .last_maximize_action_time
            .get(&ws_id)
            .map_or(false, |t| {
                t.elapsed() < std::time::Duration::from_millis(500)
            });

        let any_maximized = current_columns
            .values()
            .any(|&(w, _)| w / output_width > MAXIMIZED_RATIO_THRESHOLD);

        if !suppressed && !any_maximized && prev_widths.is_some_and(|pw| pw.len() == 2) {
            let prev = prev_widths.unwrap();

            let cols: Vec<usize> = current_columns.keys().copied().collect();
            let col_a = cols[0];
            let col_b = cols[1];

            let (cur_a, _wid_a) = current_columns[&col_a];
            let (cur_b, wid_b) = current_columns[&col_b];
            let prev_a = prev.get(&col_a).copied().unwrap_or(cur_a);
            let prev_b = prev.get(&col_b).copied().unwrap_or(cur_b);

            let delta_a = cur_a - prev_a;
            let delta_b = cur_b - prev_b;

            // Only act when exactly one column changed by a meaningful amount
            // and the other didn't (i.e. the user resized one column)
            let a_changed = delta_a.abs() > 2.0;
            let b_changed = delta_b.abs() > 2.0;

            if a_changed && !b_changed {
                let delta_px = delta_a.round() as i32;
                info!(
                    "workspace {}: column {} resized by {}px, adjusting column {} (window {})",
                    ws_id, col_a, delta_px, col_b, wid_b
                );
                let focused = self.query_focused_window().ok().flatten();
                self.send_action(Action::SetWindowWidth {
                    id: Some(wid_b),
                    change: SizeChange::AdjustFixed(-delta_px),
                })?;

                // If the left column grew, nudge viewport after layout settles
                if col_b < col_a && delta_px < 0 {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    let _ = self.send_action(Action::FocusColumnLeft {});
                    if let Some(orig) = focused {
                        let _ = self.send_action(Action::FocusWindow { id: orig });
                    }
                }

                // Update tracking to the expected state so we don't re-trigger
                let mut new_tracked = HashMap::new();
                new_tracked.insert(col_a, cur_a);
                new_tracked.insert(col_b, cur_b - delta_a);
                self.tracked_column_widths.insert(ws_id, new_tracked);
                return Ok(());
            } else if b_changed && !a_changed {
                let (_, wid_a) = current_columns[&col_a];
                let delta_px = delta_b.round() as i32;
                info!(
                    "workspace {}: column {} resized by {}px, adjusting column {} (window {})",
                    ws_id, col_b, delta_px, col_a, wid_a
                );
                let focused = self.query_focused_window().ok().flatten();
                self.send_action(Action::SetWindowWidth {
                    id: Some(wid_a),
                    change: SizeChange::AdjustFixed(-delta_px),
                })?;

                // If the left column grew, nudge viewport after layout settles
                if col_a < col_b && delta_px < 0 {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    let _ = self.send_action(Action::FocusColumnLeft {});
                    if let Some(orig) = focused {
                        let _ = self.send_action(Action::FocusWindow { id: orig });
                    }
                }

                let mut new_tracked = HashMap::new();
                new_tracked.insert(col_a, cur_a - delta_b);
                new_tracked.insert(col_b, cur_b);
                self.tracked_column_widths.insert(ws_id, new_tracked);
                return Ok(());
            }
        }

        // Update tracking baseline
        let widths: HashMap<usize, f64> = current_columns
            .iter()
            .map(|(&col, &(w, _))| (col, w))
            .collect();
        self.tracked_column_widths.insert(ws_id, widths);

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
                if self.resize_columns {
                    if let Err(e) = self.redistribute_on_column_resize(ws_id, &state, &windows_map)
                    {
                        error!(
                            "error redistributing columns for workspace {}: {:?}",
                            ws_id, e
                        );
                    }
                }
            }

            for closed_pos in &closed_positions {
                if let Some(closed_col) = closed_pos.column {
                    let min_remaining_col = self
                        .tracked_window_positions
                        .values()
                        .filter(|p| p.workspace_id == closed_pos.workspace_id)
                        .filter_map(|p| p.column)
                        .min();

                    if let Some(min_col) = min_remaining_col {
                        if closed_col > min_col {
                            debug!(
                                "closed window column {} had columns to the left, nudging viewport left",
                                closed_col
                            );
                            let target_focus = self.query_focused_window().ok().flatten();
                            let _ = self.send_action(Action::FocusColumnLeft {});
                            if let Some(orig_id) = target_focus {
                                let _ = self.send_action(Action::FocusWindow { id: orig_id });
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
