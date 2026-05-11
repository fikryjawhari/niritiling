use super::connection::{NiriConnection, NiriState, WindowPosition};
use super::manager::NiriContext;
use anyhow::Result;
use niri_ipc::{Action, Event, SizeChange, Window};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct MockState {
    pub actions: Vec<Action>,
    pub focused_window: Option<u64>,
    pub state: NiriState,
}

pub struct MockConnection {
    pub shared: Arc<Mutex<MockState>>,
}

impl NiriConnection for MockConnection {
    fn send_action(&mut self, action: Action) -> Result<()> {
        self.shared.lock().unwrap().actions.push(action);
        Ok(())
    }
    fn query_focused_window(&mut self) -> Result<Option<u64>> {
        Ok(self.shared.lock().unwrap().focused_window)
    }
    fn query_full_state(&mut self) -> Result<NiriState> {
        Ok(self.shared.lock().unwrap().state.clone())
    }
}

fn create_mock_window_at(
    id: u64,
    ws_id: u64,
    col: usize,
    tile: usize,
    width: f64,
    tile_x: f64,
) -> Window {
    use serde_json::json;
    let w_int = width as i32;
    let v = json!({
        "id": id,
        "title": "test",
        "app_id": "test",
        "workspace_id": ws_id,
        "is_focused": false,
        "is_floating": false,
        "pid": 1234,
        "is_urgent": false,
        "layout": {
            "window_size": [w_int, 0],
            "tile_pos_in_workspace_view": [tile_x, 0],
            "window_offset_in_tile": [0, 0],
            "tile_size": [w_int, 0],
            "pos_in_scrolling_layout": [col, tile]
        }
    });
    serde_json::from_value(v).expect("failed to deserialize mock window")
}

fn create_mock_window(id: u64, ws_id: u64, col: usize, tile: usize, width: f64) -> Window {
    create_mock_window_at(id, ws_id, col, tile, width, 0.0)
}

fn create_mock_fullscreen_window(id: u64, ws_id: u64, width: f64) -> Window {
    use serde_json::json;
    let w_int = width as i32;
    let v = json!({
        "id": id,
        "title": "test",
        "app_id": "test",
        "workspace_id": ws_id,
        "is_focused": false,
        "is_floating": false,
        "pid": 1234,
        "is_urgent": false,
        "layout": {
            "window_size": [w_int, 0],
            "tile_pos_in_workspace_view": [0, 0],
            "window_offset_in_tile": [0, 0],
            "tile_size": [w_int, 0],
            "pos_in_scrolling_layout": null
        }
    });
    serde_json::from_value(v).expect("failed to deserialize mock window")
}

fn setup_test(windows: Vec<Window>) -> (NiriContext, Arc<Mutex<MockState>>) {
    setup_test_with_resize(windows, false)
}

fn setup_test_with_resize(
    windows: Vec<Window>,
    resize_columns: bool,
) -> (NiriContext, Arc<Mutex<MockState>>) {
    let output_name = "eDP-1".to_string();
    let mut output_widths = HashMap::new();
    output_widths.insert(output_name.clone(), 1000.0);

    let mut ws_outputs = HashMap::new();
    ws_outputs.insert(1, output_name);

    let shared = Arc::new(Mutex::new(MockState {
        actions: Vec::new(),
        focused_window: None,
        state: NiriState {
            windows,
            output_widths,
            ws_outputs,
        },
    }));

    let conn = Box::new(MockConnection {
        shared: shared.clone(),
    });
    (NiriContext::new(conn, resize_columns), shared)
}

#[test]
fn test_opened_on_empty_maximizes() {
    let (mut ctx, shared) = setup_test(Vec::new());
    let win = create_mock_window(100, 1, 0, 0, 500.0);

    shared.lock().unwrap().state.windows.push(win.clone());

    ctx.handle_event(Event::WindowOpenedOrChanged { window: win })
        .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {}))
    );
}

#[test]
fn test_two_columns_unmaximize() {
    let win1 = create_mock_window(100, 1, 0, 0, 1000.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone()]);

    ctx.tracked_window_positions.insert(
        100,
        WindowPosition {
            workspace_id: 1,
            column: Some(0),
            tile: Some(0),
        },
    );

    let win2 = create_mock_window(101, 1, 1, 0, 1000.0);
    shared.lock().unwrap().state.windows.push(win2.clone());

    ctx.handle_event(Event::WindowOpenedOrChanged { window: win2 })
        .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {})),
        "MaximizeColumn should be sent to un-maximize"
    );
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::FocusColumnLeft {})),
        "FocusColumnLeft should be sent after un-maximizing"
    );
}

#[test]
fn test_close_one_of_two_columns_maximizes_remaining() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone()]);

    ctx.tracked_window_positions.insert(
        100,
        WindowPosition {
            workspace_id: 1,
            column: Some(0),
            tile: Some(0),
        },
    );
    ctx.tracked_window_positions.insert(
        101,
        WindowPosition {
            workspace_id: 1,
            column: Some(1),
            tile: Some(0),
        },
    );

    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    ctx.handle_event(Event::WindowClosed { id: 101 }).unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {}))
    );
}

#[test]
fn test_close_second_to_last_on_three_columns_nudges_viewport() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let win3 = create_mock_window(102, 1, 2, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone(), win3.clone()]);

    ctx.tracked_window_positions.insert(
        100,
        WindowPosition {
            workspace_id: 1,
            column: Some(0),
            tile: Some(0),
        },
    );
    ctx.tracked_window_positions.insert(
        101,
        WindowPosition {
            workspace_id: 1,
            column: Some(1),
            tile: Some(0),
        },
    );
    ctx.tracked_window_positions.insert(
        102,
        WindowPosition {
            workspace_id: 1,
            column: Some(2),
            tile: Some(0),
        },
    );

    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    ctx.handle_event(Event::WindowClosed { id: 101 }).unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::FocusColumnLeft {})),
        "FocusColumnLeft should be sent to nudge viewport left after closing middle column"
    );
}

#[test]
fn test_drag_into_column_maximizes_if_one_left() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone()]);

    ctx.tracked_window_positions.insert(
        100,
        WindowPosition {
            workspace_id: 1,
            column: Some(0),
            tile: Some(0),
        },
    );
    ctx.tracked_window_positions.insert(
        101,
        WindowPosition {
            workspace_id: 1,
            column: Some(1),
            tile: Some(0),
        },
    );

    let win2_new = create_mock_window(101, 1, 0, 1, 500.0);
    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    shared.lock().unwrap().state.windows.push(win2_new.clone());

    ctx.handle_event(Event::WindowOpenedOrChanged { window: win2_new })
        .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {}))
    );
}

#[test]
fn test_drag_out_of_column_nudges_and_unmaximizes() {
    let win1 = create_mock_window(100, 1, 0, 0, 1000.0);
    let win2 = create_mock_window(101, 1, 0, 1, 1000.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone()]);

    ctx.tracked_window_positions.insert(
        100,
        WindowPosition {
            workspace_id: 1,
            column: Some(0),
            tile: Some(0),
        },
    );
    ctx.tracked_window_positions.insert(
        101,
        WindowPosition {
            workspace_id: 1,
            column: Some(0),
            tile: Some(1),
        },
    );

    let win2_new = create_mock_window(101, 1, 1, 0, 1000.0);
    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    shared.lock().unwrap().state.windows.push(win2_new.clone());

    ctx.handle_event(Event::WindowOpenedOrChanged { window: win2_new })
        .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {}))
    );
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::FocusColumnLeft {}))
    );
}

#[test]
fn test_repro_fullscreen_move_bug() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2_tiled = create_mock_window(101, 1, 1, 0, 500.0);

    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2_tiled.clone()]);

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1.clone(), win2_tiled.clone()],
    })
    .unwrap();

    let win2_fs = create_mock_fullscreen_window(101, 1, 1000.0);
    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    shared.lock().unwrap().state.windows.push(win2_fs.clone());

    ctx.handle_event(Event::WindowOpenedOrChanged { window: win2_fs })
        .unwrap();

    shared.lock().unwrap().actions.clear();
    ctx.debounced_maximize_state.clear();

    let win2_fs_ws2 = create_mock_fullscreen_window(101, 2, 1000.0);
    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    shared
        .lock()
        .unwrap()
        .state
        .windows
        .push(win2_fs_ws2.clone());

    ctx.handle_event(Event::WindowOpenedOrChanged {
        window: win2_fs_ws2,
    })
    .unwrap();

    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    ctx.handle_event(Event::WindowClosed { id: 101 }).unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {})),
        "MaximizeColumn should have been called for win1 in WS 1"
    );
}

#[test]
fn test_repro_fullscreen_started_fs_bug() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2_fs_ws1 = create_mock_fullscreen_window(101, 1, 1000.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2_fs_ws1.clone()]);

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1.clone(), win2_fs_ws1.clone()],
    })
    .unwrap();

    let win2_fs_ws2 = create_mock_fullscreen_window(101, 2, 1000.0);
    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    shared
        .lock()
        .unwrap()
        .state
        .windows
        .push(win2_fs_ws2.clone());

    ctx.handle_event(Event::WindowOpenedOrChanged {
        window: win2_fs_ws2,
    })
    .unwrap();

    {
        let actions = &shared.lock().unwrap().actions;
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::MaximizeColumn {})),
            "WS 1 should have been re-evaluated after win2 moved away"
        );
    }

    shared.lock().unwrap().actions.clear();

    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    ctx.handle_event(Event::WindowClosed { id: 101 }).unwrap();
}

#[test]
fn test_drag_left_into_right_column_maximizes() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone()]);

    ctx.tracked_window_positions.insert(
        100,
        WindowPosition {
            workspace_id: 1,
            column: Some(0),
            tile: Some(0),
        },
    );
    ctx.tracked_window_positions.insert(
        101,
        WindowPosition {
            workspace_id: 1,
            column: Some(1),
            tile: Some(0),
        },
    );

    let win1_stacked = create_mock_window(100, 1, 0, 0, 500.0);
    let win2_stacked = create_mock_window(101, 1, 0, 1, 500.0);
    shared.lock().unwrap().state.windows = vec![win1_stacked.clone(), win2_stacked.clone()];

    ctx.handle_event(Event::WindowOpenedOrChanged {
        window: win1_stacked,
    })
    .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {})),
        "Should maximize the single remaining column after dragging left into right"
    );
}

#[test]
fn test_drag_left_into_right_column_via_layout_change() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone()]);

    ctx.tracked_window_positions.insert(
        100,
        WindowPosition {
            workspace_id: 1,
            column: Some(0),
            tile: Some(0),
        },
    );
    ctx.tracked_window_positions.insert(
        101,
        WindowPosition {
            workspace_id: 1,
            column: Some(1),
            tile: Some(0),
        },
    );

    let win1_stacked = create_mock_window(100, 1, 0, 0, 500.0);
    let win2_stacked = create_mock_window(101, 1, 0, 1, 500.0);
    shared.lock().unwrap().state.windows = vec![win1_stacked.clone(), win2_stacked.clone()];

    ctx.handle_event(Event::WindowLayoutsChanged {
        changes: vec![(100, win1_stacked.layout)],
    })
    .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {})),
        "Should maximize via layout change even when position appears unchanged"
    );
}

#[test]
fn test_stale_state_does_not_send_focus_column_left() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone()]);

    ctx.tracked_window_positions.insert(
        100,
        WindowPosition {
            workspace_id: 1,
            column: Some(0),
            tile: Some(0),
        },
    );
    ctx.tracked_window_positions.insert(
        101,
        WindowPosition {
            workspace_id: 1,
            column: Some(1),
            tile: Some(0),
        },
    );

    let win1_moved = create_mock_window(100, 1, 0, 0, 500.0);
    ctx.handle_event(Event::WindowOpenedOrChanged { window: win1_moved })
        .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::FocusColumnLeft {})),
        "FocusColumnLeft should NOT be sent when stale state shows 2 non-maximized columns"
    );
}

#[test]
fn test_second_event_after_stale_state_maximizes() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone()]);

    ctx.tracked_window_positions.insert(
        100,
        WindowPosition {
            workspace_id: 1,
            column: Some(0),
            tile: Some(0),
        },
    );
    ctx.tracked_window_positions.insert(
        101,
        WindowPosition {
            workspace_id: 1,
            column: Some(1),
            tile: Some(0),
        },
    );

    let win1_moved = create_mock_window(100, 1, 0, 0, 500.0);
    ctx.handle_event(Event::WindowOpenedOrChanged { window: win1_moved })
        .unwrap();

    shared.lock().unwrap().actions.clear();

    let win1_stacked = create_mock_window(100, 1, 0, 0, 500.0);
    let win2_stacked = create_mock_window(101, 1, 0, 1, 500.0);
    shared.lock().unwrap().state.windows = vec![win1_stacked.clone(), win2_stacked.clone()];

    ctx.handle_event(Event::WindowOpenedOrChanged {
        window: win2_stacked,
    })
    .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {})),
        "Should maximize when second event sees settled 1-column state"
    );
}

#[test]
fn test_close_rightmost_of_three_columns_nudges_viewport_left() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let win3 = create_mock_window(102, 1, 2, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone(), win3.clone()]);

    ctx.tracked_window_positions.insert(
        100,
        WindowPosition {
            workspace_id: 1,
            column: Some(0),
            tile: Some(0),
        },
    );
    ctx.tracked_window_positions.insert(
        101,
        WindowPosition {
            workspace_id: 1,
            column: Some(1),
            tile: Some(0),
        },
    );
    ctx.tracked_window_positions.insert(
        102,
        WindowPosition {
            workspace_id: 1,
            column: Some(2),
            tile: Some(0),
        },
    );

    shared.lock().unwrap().state.windows.retain(|w| w.id != 102);
    ctx.handle_event(Event::WindowClosed { id: 102 }).unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::FocusColumnLeft {})),
        "FocusColumnLeft should be sent to nudge viewport left after closing rightmost column"
    );
}

#[test]
fn test_resize_column_redistributes_two_columns() {
    let win1 = create_mock_window_at(100, 1, 0, 0, 500.0, 0.0);
    let win2 = create_mock_window_at(101, 1, 1, 0, 500.0, 500.0);
    let (mut ctx, shared) = setup_test_with_resize(vec![win1.clone(), win2.clone()], true);

    ctx.tracked_window_positions.insert(
        100,
        WindowPosition {
            workspace_id: 1,
            column: Some(0),
            tile: Some(0),
        },
    );
    ctx.tracked_window_positions.insert(
        101,
        WindowPosition {
            workspace_id: 1,
            column: Some(1),
            tile: Some(0),
        },
    );

    ctx.tracked_column_widths.insert(1, {
        let mut m = HashMap::new();
        m.insert(0, 500.0);
        m.insert(1, 500.0);
        m
    });

    // Simulate user resizing column 0 from 500 to 600
    let win1_resized = create_mock_window_at(100, 1, 0, 0, 600.0, 0.0);
    let win2_pushed = create_mock_window_at(101, 1, 1, 0, 500.0, 600.0);
    shared.lock().unwrap().state.windows = vec![win1_resized.clone(), win2_pushed];

    ctx.handle_event(Event::WindowOpenedOrChanged {
        window: win1_resized,
    })
    .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions.iter().any(|a| matches!(
            a,
            Action::SetWindowWidth {
                id: Some(101),
                change: SizeChange::AdjustFixed(-100),
            }
        )),
        "SetWindowWidth should shrink the other column by -100px: actions={:?}",
        actions
    );
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::SetWindowWidth { id: Some(100), .. })),
        "The resized column itself should NOT get a SetWindowWidth"
    );
}

#[test]
fn test_resize_does_not_cascade() {
    let win1 = create_mock_window_at(100, 1, 0, 0, 500.0, 0.0);
    let win2 = create_mock_window_at(101, 1, 1, 0, 500.0, 500.0);
    let (mut ctx, shared) = setup_test_with_resize(vec![win1.clone(), win2.clone()], true);

    ctx.tracked_window_positions.insert(
        100,
        WindowPosition {
            workspace_id: 1,
            column: Some(0),
            tile: Some(0),
        },
    );
    ctx.tracked_window_positions.insert(
        101,
        WindowPosition {
            workspace_id: 1,
            column: Some(1),
            tile: Some(0),
        },
    );

    ctx.tracked_column_widths.insert(1, {
        let mut m = HashMap::new();
        m.insert(0, 500.0);
        m.insert(1, 500.0);
        m
    });

    // First resize: column 0 grows by 100
    let win1_resized = create_mock_window_at(100, 1, 0, 0, 600.0, 0.0);
    let win2_same = create_mock_window_at(101, 1, 1, 0, 500.0, 600.0);
    shared.lock().unwrap().state.windows = vec![win1_resized.clone(), win2_same];
    ctx.handle_event(Event::WindowOpenedOrChanged {
        window: win1_resized,
    })
    .unwrap();

    shared.lock().unwrap().actions.clear();

    // Second event: both columns now at their expected new sizes (our action took effect)
    let win1_final = create_mock_window_at(100, 1, 0, 0, 600.0, 0.0);
    let win2_final = create_mock_window_at(101, 1, 1, 0, 400.0, 600.0);
    shared.lock().unwrap().state.windows = vec![win1_final.clone(), win2_final];
    ctx.handle_event(Event::WindowOpenedOrChanged { window: win1_final })
        .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::SetWindowWidth { .. })),
        "No further resize should happen after our adjustment settles: actions={:?}",
        actions
    );
}

#[test]
fn test_resize_not_triggered_with_three_columns() {
    let win1 = create_mock_window_at(100, 1, 0, 0, 333.0, 0.0);
    let win2 = create_mock_window_at(101, 1, 1, 0, 333.0, 333.0);
    let win3 = create_mock_window_at(102, 1, 2, 0, 333.0, 666.0);
    let (mut ctx, shared) =
        setup_test_with_resize(vec![win1.clone(), win2.clone(), win3.clone()], true);

    ctx.tracked_window_positions.insert(
        100,
        WindowPosition {
            workspace_id: 1,
            column: Some(0),
            tile: Some(0),
        },
    );
    ctx.tracked_window_positions.insert(
        101,
        WindowPosition {
            workspace_id: 1,
            column: Some(1),
            tile: Some(0),
        },
    );
    ctx.tracked_window_positions.insert(
        102,
        WindowPosition {
            workspace_id: 1,
            column: Some(2),
            tile: Some(0),
        },
    );

    ctx.tracked_column_widths.insert(1, {
        let mut m = HashMap::new();
        m.insert(0, 333.0);
        m.insert(1, 333.0);
        m.insert(2, 333.0);
        m
    });

    // Resize column 0 in a 3-column layout
    let win1_resized = create_mock_window_at(100, 1, 0, 0, 433.0, 0.0);
    shared.lock().unwrap().state.windows = vec![win1_resized.clone(), win2.clone(), win3.clone()];

    ctx.handle_event(Event::WindowOpenedOrChanged {
        window: win1_resized,
    })
    .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::SetWindowWidth { .. })),
        "Resize redistribution should NOT happen with 3 columns: actions={:?}",
        actions
    );
}
