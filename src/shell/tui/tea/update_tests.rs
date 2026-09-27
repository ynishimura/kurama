//! Tests for the tea state transitions and their effects.

use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

fn key_event(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn rows(names: &[&str]) -> Vec<ProfileRow> {
    names
        .iter()
        .map(|name| ProfileRow {
            name: name.to_string(),
            kind: "aws",
            session: "-".into(),
            session_expires_at: None,
            active: false,
            role_arn: None,
            region: None,
            mfa_serial: None,
            needs_human: false,
        })
        .collect()
}

fn names(rows: &[ProfileRow]) -> Vec<&str> {
    rows.iter().map(|row| row.name.as_str()).collect()
}

fn key_event_with_mod(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

#[test]
fn test_initial_model() {
    let model = TuiModel::new(false, false);
    assert_eq!(model.screen, Screen::ProfileList);
    assert!(model.profiles.loading);
    assert!(!model.should_exit);
}

#[test]
fn test_quit_message() {
    let model = TuiModel::new(false, false);
    let result = tea_update(model, TuiMessage::Key(key_event(KeyCode::Char('q'))));
    assert!(result.model.should_exit);
    assert!(matches!(result.effects.first(), Some(TuiEffect::Exit)));
}

#[test]
fn test_ctrl_c_quits() {
    let model = TuiModel::new(false, false);
    let key = key_event_with_mod(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let result = tea_update(model, TuiMessage::Key(key));
    assert!(result.model.should_exit);
}

#[test]
fn test_profile_list_navigation() {
    let mut model = TuiModel::new(false, false);
    model.profiles.loading = false;
    model.profiles.filtered_profiles = rows(&["profile1", "profile2", "profile3"]);

    // Move down
    let result = tea_update(model, TuiMessage::Key(key_event(KeyCode::Down)));
    assert_eq!(result.model.profiles.selected_index, 1);

    // Move up
    let result = tea_update(result.model, TuiMessage::Key(key_event(KeyCode::Up)));
    assert_eq!(result.model.profiles.selected_index, 0);

    // Move to end
    let result = tea_update(result.model, TuiMessage::Key(key_event(KeyCode::End)));
    assert_eq!(result.model.profiles.selected_index, 2);
}

#[test]
fn test_toggle_readonly() {
    let model = TuiModel::new(false, false);
    assert!(!model.mode.readonly);

    let result = tea_update(model, TuiMessage::Key(key_event(KeyCode::Char('r'))));
    assert!(result.model.mode.readonly);

    let result = tea_update(result.model, TuiMessage::Key(key_event(KeyCode::Char('r'))));
    assert!(!result.model.mode.readonly);
}

#[test]
fn test_help_screen_toggle() {
    let model = TuiModel::new(false, false);

    // Enter help
    let result = tea_update(model, TuiMessage::Key(key_event(KeyCode::F(1))));
    assert_eq!(result.model.screen, Screen::Help);

    // Exit help
    let result = tea_update(result.model, TuiMessage::Key(key_event(KeyCode::Esc)));
    assert_eq!(result.model.screen, Screen::ProfileList);
}

#[test]
fn test_mfa_input() {
    let mut model = TuiModel::new(false, false);
    model.screen = Screen::MfaInput;
    model.mfa_input = MfaInputModel {
        serial: "arn:aws:iam::123456789012:mfa/user".into(),
        profile: "test-profile".into(),
        value: String::new(),
    };

    // Enter digits
    let result = tea_update(model, TuiMessage::Key(key_event(KeyCode::Char('1'))));
    assert_eq!(result.model.mfa_input.value, "1");

    let result = tea_update(result.model, TuiMessage::Key(key_event(KeyCode::Char('2'))));
    assert_eq!(result.model.mfa_input.value, "12");

    // Backspace
    let result = tea_update(result.model, TuiMessage::Key(key_event(KeyCode::Backspace)));
    assert_eq!(result.model.mfa_input.value, "1");

    // Cancel
    let result = tea_update(result.model, TuiMessage::Key(key_event(KeyCode::Esc)));
    assert_eq!(result.model.screen, Screen::ProfileList);
}

#[test]
fn mfa_input_debug_redacts_code() {
    let input = MfaInputModel {
        serial: "arn:aws:iam::123456789012:mfa/user".into(),
        profile: "test-profile".into(),
        value: "999999".into(),
    };
    let debug = format!("{input:?}");

    assert!(!debug.contains("999999"));
    assert!(debug.contains("value"));
}

#[test]
fn test_profiles_loaded_success() {
    let mut model = TuiModel::new(false, false);
    model.profiles.loading = true;

    let result = tea_update(
        model,
        TuiMessage::ProfilesLoaded(ProfilesLoadedResult {
            profiles: Ok(rows(&["profile1", "profile2"])),
        }),
    );

    assert!(!result.model.profiles.loading);
    assert_eq!(result.model.profiles.all_profiles.len(), 2);
    assert_eq!(result.model.profiles.selected_index, 0);
    assert_eq!(result.model.selected_profile(), Some("profile1"));
}

#[test]
fn test_profiles_loaded_error() {
    let mut model = TuiModel::new(false, false);
    model.profiles.loading = true;

    let result = tea_update(
        model,
        TuiMessage::ProfilesLoaded(ProfilesLoadedResult {
            profiles: Err("Network error".into()),
        }),
    );

    assert!(
        matches!(result.model.screen, Screen::Error(_)),
        "{:?}",
        result.model.screen
    );
}

#[test]
fn test_assume_role_mfa_required() {
    let mut model = TuiModel::new(false, false);
    model.screen = Screen::Processing {
        message: "Testing".into(),
    };

    let result = tea_update(
        model,
        TuiMessage::AssumeRoleCompleted(AssumeRoleResult {
            profile_name: "test-profile".into(),
            result: Err(AssumeRoleError::MfaRequired {
                serial: "arn:aws:iam::123456789012:mfa/user".into(),
            }),
        }),
    );

    assert_eq!(result.model.screen, Screen::MfaInput);
    assert_eq!(result.model.mfa_input.profile, "test-profile");
}

#[test]
fn successful_assume_role_plans_credential_output() {
    let mut model = TuiModel::new(false, false);
    model.screen = Screen::Processing {
        message: "Testing".into(),
    };

    let result = tea_update(
        model,
        TuiMessage::AssumeRoleCompleted(AssumeRoleResult {
            profile_name: "test-profile".into(),
            result: Ok(super::super::messages::CredentialsInfo {
                access_key_id: "ASIA123".into(),
                credentials: crate::domain::Credentials::new(
                    "ASIA123".into(),
                    "secret".into(),
                    Some("token".into()),
                    None,
                ),
                region: None,
            }),
        }),
    );

    assert!(
        result
            .effects
            .iter()
            .any(|effect| matches!(effect, TuiEffect::WriteCredentials(_)))
    );
}

#[test]
fn profile_search_types_q_and_preserves_control_c_to_quit() {
    let mut model = TuiModel::new(false, false);
    model.profiles.loading = false;
    model.profiles.all_profiles = rows(&["dev", "qa"]);
    model.profiles.filtered_profiles = model.profiles.all_profiles.clone();
    let model = tea_update(model, TuiMessage::Key(key_event(KeyCode::Char('/')))).model;
    let result = tea_update(model, TuiMessage::Key(key_event(KeyCode::Char('q'))));
    assert!(!result.model.should_exit);
    assert_eq!(result.model.search_query.as_str(), "q");
    assert_eq!(names(&result.model.profiles.filtered_profiles), ["qa"]);
    assert!(result.effects.is_empty());
    let result = tea_update(
        result.model,
        TuiMessage::Key(key_event_with_mod(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )),
    );
    assert!(result.model.should_exit);
    assert!(matches!(result.effects.as_slice(), [TuiEffect::Exit]));
}

#[test]
fn profile_search_accepts_shift_and_edits_in_the_middle() {
    let mut model = TuiModel::new(false, false);
    model.profiles.loading = false;
    model.profiles.all_profiles = rows(&["dev", "production"]);
    model.profiles.filtered_profiles = model.profiles.all_profiles.clone();
    let mut model = tea_update(model, TuiMessage::Key(key_event(KeyCode::Char('/')))).model;
    for ch in "PRXD".chars() {
        model = tea_update(
            model,
            TuiMessage::Key(key_event_with_mod(KeyCode::Char(ch), KeyModifiers::SHIFT)),
        )
        .model;
    }
    assert_eq!(model.search_query.as_str(), "PRXD");
    for code in [
        KeyCode::Left,
        KeyCode::Left,
        KeyCode::Delete,
        KeyCode::Char('O'),
    ] {
        model = tea_update(model, TuiMessage::Key(key_event(code))).model;
    }
    assert_eq!(model.search_query.as_str(), "PROD");
    assert_eq!(names(&model.profiles.filtered_profiles), ["production"]);
    let model = tea_update(model, TuiMessage::Key(key_event(KeyCode::Left))).model;
    assert_eq!(names(&model.profiles.filtered_profiles), ["production"]);
}

#[test]
fn profile_search_filters_and_clears_profiles() {
    let mut model = TuiModel::new(false, false);
    model.profiles.loading = false;
    model.profiles.all_profiles = rows(&["dev", "production", "sandbox"]);
    model.profiles.filtered_profiles = model.profiles.all_profiles.clone();

    let result = tea_update(model, TuiMessage::Key(key_event(KeyCode::Char('/'))));
    let result = tea_update(result.model, TuiMessage::Key(key_event(KeyCode::Char('p'))));

    assert_eq!(
        names(&result.model.profiles.filtered_profiles),
        ["production"]
    );

    let result = tea_update(result.model, TuiMessage::Key(key_event(KeyCode::Backspace)));
    assert_eq!(
        names(&result.model.profiles.filtered_profiles),
        ["dev", "production", "sandbox"]
    );

    let result = tea_update(result.model, TuiMessage::Key(key_event(KeyCode::Esc)));
    assert!(!result.model.searching);
}

fn assumed(profile: &str) -> TuiMessage {
    TuiMessage::AssumeRoleCompleted(AssumeRoleResult {
        profile_name: profile.into(),
        result: Ok(super::super::messages::CredentialsInfo {
            access_key_id: "ASIA123".into(),
            credentials: crate::domain::Credentials::new(
                "ASIA123".into(),
                "secret".into(),
                Some("token".into()),
                None,
            ),
            region: None,
        }),
    })
}

fn console_url_effects(effects: &[TuiEffect]) -> Vec<(String, String, Option<String>)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            TuiEffect::GenerateConsoleUrl(effect) => Some((
                effect.credentials.access_key_id.clone(),
                effect.credentials.secret_access_key.clone(),
                effect.credentials.session_token.clone(),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn console_toggle_makes_a_successful_role_open_the_console() {
    let model = TuiModel::new(false, false);
    let off = tea_update(model.clone(), assumed("dev"));
    assert!(console_url_effects(&off.effects).is_empty());

    let toggled = tea_update(model, TuiMessage::Key(key_event(KeyCode::Char('c'))));
    assert!(toggled.model.mode.console_launch);
    let twice = tea_update(
        toggled.model.clone(),
        TuiMessage::Key(key_event(KeyCode::Char('c'))),
    );
    assert!(!twice.model.mode.console_launch);
    let result = tea_update(toggled.model, assumed("dev"));
    assert_eq!(
        console_url_effects(&result.effects),
        [(
            "ASIA123".to_string(),
            "secret".to_string(),
            Some("token".to_string())
        )]
    );

    let result = tea_update(
        result.model,
        TuiMessage::ConsoleUrlGenerated(super::super::messages::ConsoleUrlResult {
            result: Ok("https://signin.example/login?SigninToken=t".into()),
        }),
    );
    assert!(matches!(
        result.effects.as_slice(),
        [TuiEffect::OpenBrowser { url }] if url == "https://signin.example/login?SigninToken=t"
    ));
}

#[test]
fn incomplete_mfa_code_stays_on_the_prompt_until_the_sixth_digit() {
    let mut model = TuiModel::new(false, false);
    model.screen = Screen::MfaInput;
    model.mfa_input = MfaInputModel {
        serial: "arn:aws:iam::123456789012:mfa/user".into(),
        profile: "test-profile".into(),
        value: String::new(),
    };
    for digit in "12345".chars() {
        model = tea_update(model, TuiMessage::Key(key_event(KeyCode::Char(digit)))).model;
    }

    let result = tea_update(model, TuiMessage::Key(key_event(KeyCode::Enter)));
    assert_eq!(result.model.screen, Screen::MfaInput);
    assert_eq!(result.model.mfa_input.value, "12345");
    assert!(result.effects.is_empty(), "{:?}", result.effects);

    let model = tea_update(result.model, TuiMessage::Key(key_event(KeyCode::Char('6')))).model;
    let result = tea_update(model, TuiMessage::Key(key_event(KeyCode::Enter)));
    assert!(matches!(result.model.screen, Screen::Processing { .. }));
    assert!(matches!(
        result.effects.as_slice(),
        [TuiEffect::AssumeRole(AssumeRoleEffect { profile_name, mfa_token: Some(code), .. })]
            if profile_name == "test-profile" && code == "123456"
    ));
}

#[test]
fn a_browser_that_fails_to_open_shows_the_url_and_keeps_the_tui() {
    let model = tea_update(TuiModel::new(false, true), assumed("dev")).model;
    let result = tea_update(
        model.clone(),
        TuiMessage::BrowserOpened(super::super::messages::BrowserOpenResult {
            result: Err("`open` exited with exit status: 1".into()),
        }),
    );
    assert!(!result.model.should_exit);
    assert!(result.effects.is_empty(), "{:?}", result.effects);
    assert_eq!(
        result.model.screen,
        Screen::Error(ErrorModel {
            message: "Failed to open the browser: `open` exited with exit status: 1".into(),
            console_url_on_quit: true,
        })
    );

    let opened = tea_update(
        model,
        TuiMessage::BrowserOpened(super::super::messages::BrowserOpenResult { result: Ok(()) }),
    );
    assert!(matches!(opened.model.screen, Screen::Success(_)));
    assert!(opened.effects.is_empty(), "{:?}", opened.effects);
}

#[test]
fn a_tick_moves_the_clock_the_header_reads() {
    let now = chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap();
    let result = tea_update(TuiModel::new(false, false), TuiMessage::Tick(now));
    assert_eq!(result.model.now, now);
    assert!(result.effects.is_empty(), "{:?}", result.effects);
}

mod tabs {
    use super::*;
    use crate::shell::tui::tea::sources::{Enter, SourceRow, Tab};
    use crate::shell::tui::testing::fixtures;

    fn press(model: TuiModel, code: KeyCode) -> UpdateResult {
        tea_update(model, TuiMessage::Key(key_event(code)))
    }

    #[test]
    fn digits_and_tab_switch_tabs_without_an_effect() {
        let mut model = fixtures::loaded();
        model.sources = fixtures::sources();
        for (code, tab) in [
            (KeyCode::Char('3'), Tab::Api),
            (KeyCode::Tab, Tab::Db),
            (KeyCode::Tab, Tab::Data),
            (KeyCode::Tab, Tab::S3),
            (KeyCode::Tab, Tab::Aws),
            (KeyCode::BackTab, Tab::S3),
            (KeyCode::Char('6'), Tab::S3),
            (KeyCode::Char('2'), Tab::Auth),
            (KeyCode::Char('1'), Tab::Aws),
        ] {
            let result = press(model, code);
            assert_eq!(result.model.tab, tab, "{code:?}");
            assert!(result.effects.is_empty(), "{code:?}: {:?}", result.effects);
            model = result.model;
        }
        // The AWS tab keeps its keys and its selection.
        let result = press(model, KeyCode::Down);
        assert_eq!(result.model.profiles.selected_index, 1);
    }

    #[test]
    fn a_digit_typed_into_the_search_stays_in_the_search() {
        let model = press(fixtures::loaded(), KeyCode::Char('/')).model;
        let result = press(model, KeyCode::Char('2'));
        assert_eq!(result.model.tab, Tab::Aws);
        assert_eq!(result.model.search_query.as_str(), "2");
    }

    #[test]
    fn a_source_tab_moves_its_own_selection_and_leaves_aws_alone() {
        let mut model = fixtures::loaded();
        model.sources = fixtures::sources();
        let model = press(model, KeyCode::Char('2')).model;
        let model = press(model, KeyCode::Down).model;
        assert_eq!(model.sources.auth.selected, 1);
        assert_eq!(model.profiles.selected_index, 0);
        // `/`, `r` and `c` belong to the AWS tab.
        let model = press(model, KeyCode::Char('/')).model;
        assert!(!model.searching);
        let model = press(model, KeyCode::Char('r')).model;
        assert!(!model.mode.readonly);
    }

    #[test]
    fn enter_on_an_api_with_a_description_hands_off_to_the_explorer() {
        let mut model = fixtures::loaded();
        model.sources = fixtures::sources();
        let model = press(model, KeyCode::Char('3')).model;
        let result = press(model, KeyCode::Enter);
        assert_eq!(
            result.model.handoff,
            Some(Handoff::Run(vec!["api".to_string(), "github".to_string()]))
        );
        assert!(result.model.should_exit);
        assert!(matches!(result.effects.as_slice(), [TuiEffect::Exit]));
    }

    #[test]
    fn enter_on_an_api_without_a_description_says_why_and_stays() {
        let mut model = fixtures::loaded();
        model.sources = fixtures::sources();
        model.tab = Tab::Api;
        model.sources.api.selected = 1;
        let result = press(model, KeyCode::Enter);
        assert_eq!(result.model.handoff, None);
        assert!(!result.model.should_exit);
        assert_eq!(
            result.model.notice.as_deref(),
            Some("[api.internal] has no openapi description to explore")
        );
        // The next key clears it.
        let result = press(result.model, KeyCode::Up);
        assert_eq!(result.model.notice, None);
    }

    #[test]
    fn enter_on_a_row_that_copies_asks_for_the_clipboard_and_reports_it() {
        let mut model = fixtures::loaded();
        model.tab = Tab::Data;
        model.sources.data.rows = vec![SourceRow {
            name: "logs".into(),
            kind: "data",
            state: "not_checked".into(),
            active: false,
            details: Vec::new(),
            next: String::new(),
            enter: Enter::Copy("kurama data logs --tables".into()),
        }];
        let result = press(model, KeyCode::Enter);
        assert!(matches!(
            result.effects.as_slice(),
            [TuiEffect::CopyToClipboard { text }] if text == "kurama data logs --tables"
        ));
        let result = tea_update(result.model, TuiMessage::Copied(Ok(())));
        assert_eq!(
            result.model.notice.as_deref(),
            Some("command copied to the clipboard")
        );
    }
}

mod palette {
    use super::*;
    use crate::domain::types::request_history::HistoryEntry;
    use crate::shell::tui::tea::palette::{PaletteItem, PaletteTarget};
    use crate::shell::tui::tea::sources::Tab;
    use crate::shell::tui::testing::fixtures;

    fn press_with(model: TuiModel, code: KeyCode, modifiers: KeyModifiers) -> UpdateResult {
        tea_update(model, TuiMessage::Key(KeyEvent::new(code, modifiers)))
    }

    fn press(model: TuiModel, code: KeyCode) -> UpdateResult {
        press_with(model, code, KeyModifiers::NONE)
    }

    fn typed(mut model: TuiModel, text: &str) -> TuiModel {
        for c in text.chars() {
            model = press(model, KeyCode::Char(c)).model;
        }
        model
    }

    fn operation(api: &str, id: &str) -> PaletteItem {
        PaletteItem::new(
            "op",
            id.into(),
            api.into(),
            PaletteTarget::Explore {
                api: api.into(),
                entry: Some(HistoryEntry {
                    operation: id.into(),
                    params: Vec::new(),
                    body: None,
                    name: None,
                }),
            },
        )
    }

    fn with_sources() -> TuiModel {
        let mut model = fixtures::loaded();
        model.sources = fixtures::sources();
        model
    }

    #[test]
    fn ctrl_k_or_colon_opens_it_and_reads_the_operations_once() {
        let result = press_with(with_sources(), KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert!(result.model.palette.is_some());
        assert!(matches!(
            result.effects.as_slice(),
            [TuiEffect::LoadPaletteItems]
        ));
        let model = tea_update(
            result.model,
            TuiMessage::PaletteItemsLoaded(vec![operation("pets", "pets/get")]),
        )
        .model;
        let palette = model.palette.as_ref().unwrap();
        assert!(
            palette
                .matches
                .iter()
                .any(|index| palette.items[*index].name == "pets/get"),
            "the operations join the open palette"
        );
        let model = press(model, KeyCode::Esc).model;
        assert!(model.palette.is_none());
        let result = press(model, KeyCode::Char(':'));
        assert!(result.model.palette.is_some());
        assert!(result.effects.is_empty(), "read once: {:?}", result.effects);
        // q is typed into the query, not a quit.
        let result = press(result.model, KeyCode::Char('q'));
        assert!(!result.model.should_exit);
        assert_eq!(result.model.palette.unwrap().input.as_str(), "q");
    }

    #[test]
    fn an_operation_hands_off_to_its_form_and_a_profile_is_selected() {
        let model = press_with(with_sources(), KeyCode::Char('k'), KeyModifiers::CONTROL).model;
        let model = tea_update(
            model,
            TuiMessage::PaletteItemsLoaded(vec![operation("pets", "pets/get")]),
        )
        .model;
        let result = press(typed(model, "pets/get"), KeyCode::Enter);
        assert_eq!(
            result.model.handoff,
            Some(Handoff::Explore {
                api: "pets".into(),
                entry: HistoryEntry {
                    operation: "pets/get".into(),
                    params: Vec::new(),
                    body: None,
                    name: None,
                },
            })
        );
        assert!(result.model.should_exit);
        assert!(matches!(result.effects.as_slice(), [TuiEffect::Exit]));

        let model = press_with(with_sources(), KeyCode::Char('k'), KeyModifiers::CONTROL).model;
        let model = press(typed(model, "sandbox"), KeyCode::Enter).model;
        assert!(model.palette.is_none());
        assert!(!model.should_exit);
        assert_eq!(model.tab, Tab::Aws);
        assert_eq!(model.selected_profile(), Some("sandbox-mfa"));

        let model = press_with(with_sources(), KeyCode::Char('k'), KeyModifiers::CONTROL).model;
        let result = press(typed(model, "github login"), KeyCode::Enter);
        assert_eq!(result.model.tab, Tab::Auth);
        assert_eq!(
            result.model.handoff, None,
            "an auth row is selected, not run"
        );
    }
}
