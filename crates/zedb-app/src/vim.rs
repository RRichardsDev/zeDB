use std::collections::VecDeque;
use std::str::FromStr;

use modalkit::{
    actions::{Action, CommandBarAction, Editable, MacroAction, PromptAction, Searchable},
    editing::{
        application::EmptyInfo,
        buffer::{CursorGroupId, EditBuffer},
        context::{EditContext, Resolve},
        cursor::Cursor,
        rope::EditRope,
        store::{RegisterPutFlags, Store},
    },
    env::vim::{
        command::VimCommandMachine,
        keybindings::{default_vim_keys, VimMachine},
        VimMode,
    },
    key::TerminalKey,
    keybindings::{BindingMachine, InputKey},
    prelude::{CommandType, Register, TargetShape, ViewportContext},
};

// Macro replay depth guard, mirroring modalkit's KeyManager. That wrapper is
// not used directly because boxing the machine hides ModalMachine::mode().
const MAX_MACRO_EXEC_DEPTH: usize = 100;

pub struct VimController {
    machine: VimMachine<TerminalKey>,
    buffer: EditBuffer<EmptyInfo>,
    cursor_group: CursorGroupId,
    viewport: ViewportContext<Cursor>,
    store: Store<EmptyInfo>,
    commands: VimCommandMachine,
    cmdline: Option<CommandLine>,
    keystack: VecDeque<TerminalKey>,
    recording: Option<(Register, bool)>,
    commit_on_input: bool,
    committed: EditRope,
    pending: EditRope,
    macro_exec_depth: usize,
    preserve_trailing_newline: bool,
    unsupported: Option<String>,
}

struct CommandLine {
    buffer: EditBuffer<EmptyInfo>,
    cursor_group: CursorGroupId,
    prompt: String,
    kind: CommandType,
    action: Action<EmptyInfo>,
    context: EditContext,
}

pub struct VimSnapshot {
    pub text: String,
    pub line: usize,
    pub column: usize,
    pub selection: Option<VimSelection>,
    pub command_line: Option<CommandLineSnapshot>,
    pub recording: Option<char>,
    pub unsupported: Option<String>,
}

/// A visual-mode selection in (line, column) character coordinates,
/// with `start <= end` and `end` exclusive.
pub struct VimSelection {
    pub start: (usize, usize),
    pub end: (usize, usize),
}

#[derive(Clone)]
pub struct CommandLineSnapshot {
    pub prompt: String,
    pub text: String,
    pub cursor: usize,
}

impl VimController {
    pub fn new(text: &str) -> Self {
        let mut buffer = EditBuffer::from_str(String::new(), text);
        let cursor_group = buffer.create_group();
        Self {
            machine: default_vim_keys(),
            buffer,
            cursor_group,
            viewport: ViewportContext::default(),
            store: Store::default(),
            commands: VimCommandMachine::default(),
            cmdline: None,
            keystack: VecDeque::new(),
            recording: None,
            commit_on_input: false,
            committed: EditRope::from(""),
            pending: EditRope::from(""),
            macro_exec_depth: 0,
            preserve_trailing_newline: text.ends_with('\n'),
            unsupported: None,
        }
    }

    pub fn mode(&self) -> VimMode {
        self.machine.mode()
    }

    pub fn mode_label(&self) -> &'static str {
        match self.machine.mode() {
            VimMode::Normal => "NORMAL",
            VimMode::Insert => "INSERT",
            VimMode::Visual => match self.leader_shape() {
                TargetShape::CharWise => "VISUAL",
                TargetShape::LineWise => "V-LINE",
                TargetShape::BlockWise => "V-BLOCK",
            },
            VimMode::Select => "SELECT",
            VimMode::OperationPending => "OPERATOR",
            VimMode::Command => "COMMAND",
            VimMode::LangArg | VimMode::CharReplaceSuffix | VimMode::CharSearchSuffix => "NORMAL",
        }
    }

    pub fn recording_indicator(&self) -> Option<char> {
        self.recording.as_ref().map(|(register, _)| match register {
            Register::Named(name) => *name,
            _ => '@',
        })
    }

    pub fn command_line(&mut self) -> Option<CommandLineSnapshot> {
        let cmdline = self.cmdline.as_mut()?;
        let mut text = cmdline.buffer.get_text();
        if text.ends_with('\n') {
            text.pop();
        }
        let cursor = cmdline.buffer.get_leader(cmdline.cursor_group);
        Some(CommandLineSnapshot {
            prompt: cmdline.prompt.clone(),
            text,
            cursor: cursor.x,
        })
    }

    fn leader_shape(&self) -> TargetShape {
        self.buffer
            .get_group_selections(self.cursor_group)
            .and_then(|selections| selections.first().map(|(_, _, shape)| *shape))
            .unwrap_or(TargetShape::CharWise)
    }

    fn visual_selection(&mut self) -> Option<VimSelection> {
        if !matches!(self.machine.mode(), VimMode::Visual | VimMode::Select) {
            return None;
        }
        let (start, end, shape) = self.buffer.get_leader_selection(self.cursor_group)?;
        match shape {
            TargetShape::CharWise => Some(VimSelection {
                start: (start.y, start.x),
                end: (end.y, end.x + 1),
            }),
            TargetShape::LineWise => Some(VimSelection {
                start: (start.y, 0),
                end: (end.y + 1, 0),
            }),
            // A rectangular selection cannot be expressed as the editor's
            // single linear range; the status bar still reports V-BLOCK.
            TargetShape::BlockWise => None,
        }
    }

    pub fn set_cursor(&mut self, line: usize, column: usize) {
        // set_leader stores the position unchecked and modalkit's edit
        // operations index the rope with it, so an out-of-bounds position
        // becomes a panic on the next edit.
        let line = line.min(self.buffer.get_lines().saturating_sub(1));
        let column = column.min(self.buffer.get_columns(line));
        self.buffer
            .set_leader(self.cursor_group, Cursor::new(line, column));
    }

    pub fn reset(&mut self, text: &str, line: usize, column: usize) {
        *self = Self::new(text);
        self.set_cursor(line, column);
    }

    pub fn input(&mut self, key: &str) -> Result<VimSnapshot, String> {
        let key = TerminalKey::from_str(key).map_err(|error| error.to_string())?;
        self.macro_exec_depth = 0;
        if self.recording.is_some() {
            let mut rope = EditRope::from(key.to_string());
            if self.commit_on_input {
                std::mem::swap(&mut self.pending, &mut rope);
                self.committed += rope;
                self.commit_on_input = false;
            } else {
                self.pending += rope;
            }
        }
        self.keystack.clear();
        self.machine.input_key(key);
        self.drain_actions()?;
        Ok(self.snapshot())
    }

    fn drain_actions(&mut self) -> Result<(), String> {
        while let Some((action, context)) = self.next_action() {
            self.apply_action(action, context)?;
        }
        Ok(())
    }

    /// Pop the next action, feeding queued macro keys into the machine as
    /// needed (same replay loop as modalkit's KeyManager).
    fn next_action(&mut self) -> Option<(Action<EmptyInfo>, EditContext)> {
        loop {
            if let Some(pair) = self.machine.pop() {
                self.commit_on_input = true;
                return Some(pair);
            }
            match self.keystack.pop_front() {
                Some(key) => self.machine.input_key(key),
                None => return None,
            }
        }
    }

    fn apply_action(
        &mut self,
        action: Action<EmptyInfo>,
        context: EditContext,
    ) -> Result<(), String> {
        match action {
            Action::NoOp => {}
            Action::Editor(action) => {
                if let Some(cmdline) = &mut self.cmdline {
                    let context = (cmdline.cursor_group, &self.viewport, &context);
                    cmdline
                        .buffer
                        .editor_command(&action, &context, &mut self.store)
                        .map_err(|error| error.to_string())?;
                } else {
                    let context = (self.cursor_group, &self.viewport, &context);
                    self.buffer
                        .editor_command(&action, &context, &mut self.store)
                        .map_err(|error| error.to_string())?;
                }
            }
            Action::Search(dir, count) => {
                let context = (self.cursor_group, &self.viewport, &context);
                self.buffer
                    .search(dir, count, &context, &mut self.store)
                    .map_err(|error| error.to_string())?;
            }
            Action::CommandBar(CommandBarAction::Focus(prompt, kind, action)) => {
                let mut buffer = EditBuffer::from_str(String::new(), "");
                let cursor_group = buffer.create_group();
                self.cmdline = Some(CommandLine {
                    buffer,
                    cursor_group,
                    prompt: prompt.to_string(),
                    kind,
                    action: *action,
                    context,
                });
            }
            Action::CommandBar(CommandBarAction::Unfocus) => self.cmdline = None,
            Action::Prompt(PromptAction::Abort(_)) => self.cmdline = None,
            Action::Prompt(PromptAction::Submit) => self.submit_command_line()?,
            Action::Repeat(sequence) => self.machine.repeat(sequence, Some(context)),
            Action::Macro(action) => self.macro_command(&action, &context)?,
            action => self.unsupported = Some(format!("{action:?}")),
        }
        Ok(())
    }

    fn submit_command_line(&mut self) -> Result<(), String> {
        let Some(cmdline) = self.cmdline.take() else {
            return Ok(());
        };
        let mut text = cmdline.buffer.get_text();
        if text.ends_with('\n') {
            text.pop();
        }
        match cmdline.kind {
            CommandType::Search => {
                if !text.is_empty() {
                    self.store.registers.set_last_search(text);
                }
                self.apply_action(cmdline.action, cmdline.context)
            }
            CommandType::Command => {
                let actions = self
                    .commands
                    .input_cmd(text, cmdline.context)
                    .map_err(|error| error.to_string())?;
                for (action, context) in actions {
                    self.apply_action(action, context)?;
                }
                Ok(())
            }
        }
    }

    /// Macro recording and replay, mirroring modalkit's KeyManager.
    fn macro_command(&mut self, act: &MacroAction, ctx: &EditContext) -> Result<(), String> {
        let (mstr, count) = match act {
            MacroAction::Execute(count) => {
                let reg = ctx.get_register().unwrap_or(Register::UnnamedMacro);
                let rope = self
                    .store
                    .registers
                    .get_macro(reg)
                    .map_err(|error| error.to_string())?;
                (rope.to_string(), ctx.resolve(count))
            }
            MacroAction::Run(mstr, count) => (mstr.clone(), ctx.resolve(count)),
            MacroAction::Repeat(count) => {
                let rope = self
                    .store
                    .registers
                    .get_last_macro()
                    .map_err(|error| error.to_string())?;
                (rope.to_string(), ctx.resolve(count))
            }
            MacroAction::ToggleRecording => {
                if let Some((register, append)) = self.recording.take() {
                    let mut rope = EditRope::from("");
                    std::mem::swap(&mut rope, &mut self.committed);
                    let mut flags = RegisterPutFlags::NOTEXT;
                    if append {
                        flags |= RegisterPutFlags::APPEND;
                    }
                    self.store
                        .registers
                        .put(&register, rope.into(), flags)
                        .map_err(|error| error.to_string())?;
                    self.commit_on_input = false;
                    self.pending = EditRope::from("");
                } else {
                    let register = ctx.get_register().unwrap_or(Register::UnnamedMacro);
                    self.recording = Some((register, ctx.get_register_append()));
                }
                return Ok(());
            }
            act => return Err(format!("unsupported macro action: {act:?}")),
        };

        self.macro_exec_depth += 1;
        if self.macro_exec_depth >= MAX_MACRO_EXEC_DEPTH {
            return Err(format!("macro loops (depth {})", self.macro_exec_depth));
        }
        for _ in 0..count {
            let mut keys =
                VecDeque::from(TerminalKey::from_macro_str(mstr.as_ref()).map_err(|error| {
                    error.to_string()
                })?);
            keys.append(&mut self.keystack);
            self.keystack = keys;
        }
        Ok(())
    }

    pub fn snapshot(&mut self) -> VimSnapshot {
        let cursor = self.buffer.get_leader(self.cursor_group);
        let selection = self.visual_selection();
        let command_line = self.command_line();
        let recording = self.recording_indicator();
        let mut text = self.buffer.get_text();
        if !self.preserve_trailing_newline && text.ends_with('\n') {
            text.pop();
        }
        VimSnapshot {
            text,
            line: cursor.y,
            column: cursor.x,
            selection,
            command_line,
            recording,
            unsupported: self.unsupported.take(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_at_end_of_buffer_round_trip() {
        use gpui_component::input::{Position, RopeExt};
        use gpui_component::Rope;
        let mut vim = VimController::new("one\ntwo");
        vim.set_cursor(1, 3);
        let mut snap = vim.input("i").unwrap();
        for key in ["<Enter>", "<Enter>", "x"] {
            // mimic the editor: clamp the snapshot cursor through gpui-component rope math
            let rope = Rope::from(snap.text.as_str());
            let offset =
                rope.position_to_offset(&Position::new(snap.line as u32, snap.column as u32));
            let clamped = rope.offset_to_position(offset);
            vim.set_cursor(clamped.line as usize, clamped.character as usize);
            snap = vim.input(key).unwrap();
        }
        assert_eq!(snap.text, "one\ntwo\n\nx");
    }

    #[test]
    fn out_of_bounds_cursor_is_clamped_before_edits() {
        let mut vim = VimController::new("abc\ndef");
        vim.set_cursor(99, 99);
        vim.input("i").unwrap();
        let inserted = vim.input("x").unwrap();
        assert_eq!(inserted.text, "abc\ndefx");
        assert_eq!((inserted.line, inserted.column), (1, 4));
    }

    #[test]
    fn charwise_visual_selection_is_reported() {
        let mut vim = VimController::new("alpha beta\ngamma");
        vim.input("v").unwrap();
        let snapshot = vim.input("e").unwrap();
        let selection = snapshot.selection.expect("visual mode reports a selection");
        assert_eq!(selection.start, (0, 0));
        assert_eq!(selection.end, (0, 5));
        assert_eq!(vim.mode_label(), "VISUAL");

        let escaped = vim.input("<Esc>").unwrap();
        assert!(escaped.selection.is_none());
        assert_eq!(vim.mode_label(), "NORMAL");
    }

    #[test]
    fn linewise_visual_selection_spans_whole_lines() {
        let mut vim = VimController::new("one\ntwo\nthree");
        vim.input("V").unwrap();
        assert_eq!(vim.mode_label(), "V-LINE");
        let snapshot = vim.input("j").unwrap();
        let selection = snapshot.selection.expect("line-visual reports a selection");
        assert_eq!(selection.start, (0, 0));
        assert_eq!(selection.end, (2, 0));
    }

    #[test]
    fn blockwise_visual_selection_is_labeled_but_not_ranged() {
        let mut vim = VimController::new("one\ntwo\nthree");
        vim.input("<C-v>").unwrap();
        assert_eq!(vim.mode_label(), "V-BLOCK");
        let snapshot = vim.input("j").unwrap();
        assert!(snapshot.selection.is_none());
    }

    #[test]
    fn counts_and_line_delete_are_executed_by_modalkit() {
        let mut vim = VimController::new("one\ntwo\nthree\nfour\nfive\nsix");
        vim.input("5").unwrap();
        let moved = vim.input("j").unwrap();
        assert_eq!(moved.line, 5);

        vim.set_cursor(0, 0);
        vim.input("d").unwrap();
        let deleted = vim.input("d").unwrap();
        assert_eq!(deleted.text, "two\nthree\nfour\nfive\nsix");
    }

    #[test]
    fn insert_mode_is_driven_by_modalkit() {
        let mut vim = VimController::new("SELECT 1;");
        vim.input("i").unwrap();
        assert_eq!(vim.mode(), VimMode::Insert);
        vim.input("X").unwrap();
        let normal = vim.input("<Esc>").unwrap();
        assert_eq!(vim.mode(), VimMode::Normal);
        assert_eq!(normal.text, "XSELECT 1;");
    }

    #[test]
    fn operators_and_history_are_executed_by_modalkit() {
        let mut vim = VimController::new("alpha beta gamma");

        vim.input("d").unwrap();
        let deleted = vim.input("w").unwrap();
        assert_eq!(deleted.text, "beta gamma");

        let undone = vim.input("u").unwrap();
        assert_eq!(undone.text, "alpha beta gamma");

        let redone = vim.input("<C-r>").unwrap();
        assert_eq!(redone.text, "beta gamma");
    }

    #[test]
    fn line_registers_and_repeat_are_executed_by_modalkit() {
        let mut vim = VimController::new("alpha\nbeta");
        vim.input("y").unwrap();
        vim.input("y").unwrap();
        let pasted = vim.input("p").unwrap();
        assert_eq!(pasted.text, "alpha\nalpha\nbeta");

        let repeated = vim.input(".").unwrap();
        assert_eq!(repeated.text, "alpha\nalpha\nalpha\nbeta");
    }

    #[test]
    fn visual_edits_and_trailing_newlines_are_preserved() {
        let mut vim = VimController::new("first\nsecond\n");
        vim.input("v").unwrap();
        assert_eq!(vim.mode(), VimMode::Visual);
        vim.input("e").unwrap();
        let deleted = vim.input("d").unwrap();
        assert_eq!(vim.mode(), VimMode::Normal);
        assert_eq!(deleted.text, "\nsecond\n");
    }

    #[test]
    fn forward_search_targets_command_line_not_buffer() {
        let mut vim = VimController::new("one two\nthree two");
        vim.input("/").unwrap();
        assert_eq!(vim.mode(), VimMode::Command);

        let typed = vim.input("t").unwrap();
        // The typed pattern goes to the command line, never the buffer.
        assert_eq!(typed.text, "one two\nthree two");
        let cmdline = typed.command_line.expect("command line is active");
        assert_eq!(cmdline.prompt, "/");
        assert_eq!(cmdline.text, "t");

        vim.input("w").unwrap();
        vim.input("o").unwrap();
        let submitted = vim.input("<Enter>").unwrap();
        assert!(submitted.command_line.is_none());
        assert_eq!((submitted.line, submitted.column), (0, 4));

        let next = vim.input("n").unwrap();
        assert_eq!((next.line, next.column), (1, 6));
    }

    #[test]
    fn backward_search_and_abort() {
        let mut vim = VimController::new("alpha\nbeta\nalpha");
        vim.set_cursor(2, 0);
        vim.input("?").unwrap();
        for key in ["a", "l", "p", "h", "a"] {
            vim.input(key).unwrap();
        }
        let submitted = vim.input("<Enter>").unwrap();
        assert_eq!((submitted.line, submitted.column), (0, 0));

        vim.input("?").unwrap();
        let aborted = vim.input("<Esc>").unwrap();
        assert!(aborted.command_line.is_none());
        assert_eq!(vim.mode(), VimMode::Normal);
        assert_eq!(aborted.text, "alpha\nbeta\nalpha");
    }

    #[test]
    fn macros_record_and_replay() {
        let mut vim = VimController::new("abcdef");
        let recording = vim.input("q").and_then(|_| vim.input("a")).unwrap();
        assert_eq!(recording.recording, Some('a'));
        vim.input("x").unwrap();
        let stopped = vim.input("q").unwrap();
        assert!(stopped.recording.is_none());

        vim.input("@").unwrap();
        let replayed = vim.input("a").unwrap();
        assert_eq!(replayed.text, "cdef");

        vim.input("2").unwrap();
        vim.input("@").unwrap();
        let repeated = vim.input("a").unwrap();
        assert_eq!(repeated.text, "ef");
    }

    #[test]
    fn ex_commands_parse_and_report_unimplemented_explicitly() {
        // modalkit parses ex commands but has not implemented substitution;
        // the app must surface that as an explicit error, not silence.
        let mut vim = VimController::new("alpha beta\nalpha");
        vim.input(":").unwrap();
        for key in ["s", "/", "a", "l", "p", "h", "a", "/", "o", "m", "e", "g", "a", "/"] {
            vim.input(key).unwrap();
        }
        let error = vim.input("<Enter>").map(|_| ()).unwrap_err();
        assert!(error.contains("substitution is not yet implemented"));
        assert_eq!(vim.snapshot().text, "alpha beta\nalpha");
    }
}
