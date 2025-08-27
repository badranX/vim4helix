// This file is a hack and should be removed once Helix introduce alternative solution
// It is adapted from shell commands of commands::*
use crate::commands::*;

fn shell_impl(shell: &[String], cmd: &str, input: Option<Rope>) -> anyhow::Result<Tendril> {
    tokio::task::block_in_place(|| helix_lsp::block_on(shell_impl_async(shell, cmd, input)))
}

async fn shell_impl_async(
    shell: &[String],
    cmd: &str,
    input: Option<Rope>,
) -> anyhow::Result<Tendril> {
    use std::process::Stdio;
    use tokio::process::Command;
    ensure!(!shell.is_empty(), "No shell set");

    let mut process = Command::new(&shell[0]);
    process
        .args(&shell[1..])
        .arg(cmd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if input.is_some() || cfg!(windows) {
        process.stdin(Stdio::piped());
    } else {
        process.stdin(Stdio::null());
    }

    let mut process = match process.spawn() {
        Ok(process) => process,
        Err(e) => {
            log::error!("Failed to start shell: {}", e);
            return Err(e.into());
        }
    };
    let output = if let Some(mut stdin) = process.stdin.take() {
        let input_task = tokio::spawn(async move {
            if let Some(input) = input {
                helix_view::document::to_writer(&mut stdin, (encoding::UTF_8, false), &input)
                    .await?;
            }
            anyhow::Ok(())
        });
        let (output, _) = tokio::join! {
            process.wait_with_output(),
            input_task,
        };
        output?
    } else {
        // Process has no stdin, so we just take the output
        process.wait_with_output().await?
    };

    let output = if !output.status.success() {
        match output.status.code() {
            Some(exit_code) => bail!("Shell command failed: status {}", exit_code),
            None => bail!("Shell command failed"),
        }
        // String::from_utf8_lossy(&output.stderr)
        // Prioritize `stderr` output over `stdout`
    } else if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::debug!("Command printed to stderr: {stderr}");
        stderr
    } else {
        String::from_utf8_lossy(&output.stdout)
    };

    Ok(Tendril::from(output))
}

pub fn shell_explicit(
    cx: &mut compositor::Context,
    cmd: &str,
    input_range: Range,
    prev_range: Range,
) {
    // Adapted from commands::shell
    let pipe = true;

    let config = cx.editor.config();
    let shell = &config.shell;
    let (view, doc) = current!(cx.editor);
    let selection = doc.selection(view.id);

    let mut changes = Vec::with_capacity(selection.len());
    let text = doc.text().slice(..);

    let output = {
        let input = input_range.slice(text);
        match shell_impl(shell, cmd, pipe.then(|| input.into())) {
            Ok(mut output) => {
                if !input.ends_with("\n") && output.ends_with('\n') {
                    output.pop();
                    if output.ends_with('\r') {
                        output.pop();
                    }
                }

                output
            }
            Err(err) => {
                cx.editor.set_error(err.to_string());
                return;
            }
        }
    };

    let (from, to) = (input_range.from(), input_range.to());

    let len_output = output.len();
    changes.push((from, to, Some(output)));

    let new_pos = prev_range.from().min(len_output);
    let prev_selection = Selection::single(new_pos, new_pos);

    let transaction =
        Transaction::change(doc.text(), changes.into_iter()).with_selection(prev_selection);
    doc.apply(&transaction, view.id);
    doc.append_changes_to_history(view);

    // after replace cursor may be out of bounds, do this to
    // make sure cursor is in view and update scroll as well
    view.ensure_cursor_in_view(doc, config.scrolloff);
}

pub fn shell_on_success(cx: &mut compositor::Context, cmd: &str) {
    // Adapted from commands::shell
    let pipe = true;

    let config = cx.editor.config();
    let shell = &config.shell;
    let (view, doc) = current!(cx.editor);
    let selection = doc.selection(view.id);

    let mut changes = Vec::with_capacity(selection.len());
    let mut ranges = SmallVec::with_capacity(selection.len());
    let text = doc.text().slice(..);

    let mut shell_output: Option<Tendril> = None;
    let mut offset = 0isize;
    for range in selection.ranges() {
        let output = if let Some(output) = shell_output.as_ref() {
            output.clone()
        } else {
            let input = range.slice(text);
            match shell_impl(shell, cmd, pipe.then(|| input.into())) {
                Ok(mut output) => {
                    if !input.ends_with("\n") && output.ends_with('\n') {
                        output.pop();
                        if output.ends_with('\r') {
                            output.pop();
                        }
                    }

                    if !pipe {
                        shell_output = Some(output.clone());
                    }
                    output
                }
                Err(err) => {
                    cx.editor.set_error(err.to_string());
                    return;
                }
            }
        };

        let output_len = output.chars().count();

        let (from, to, deleted_len) = (range.from(), range.to(), range.len());

        // These `usize`s cannot underflow because selection ranges cannot overlap.
        let anchor = to
            .checked_add_signed(offset)
            .expect("Selection ranges cannot overlap")
            .checked_sub(deleted_len)
            .expect("Selection ranges cannot overlap");
        let new_range = Range::new(anchor, anchor + output_len).with_direction(range.direction());
        ranges.push(new_range);
        offset = offset
            .checked_add_unsigned(output_len)
            .expect("Selection ranges cannot overlap")
            .checked_sub_unsigned(deleted_len)
            .expect("Selection ranges cannot overlap");

        changes.push((from, to, Some(output)));
    }

    let transaction = Transaction::change(doc.text(), changes.into_iter())
        .with_selection(Selection::new(ranges, selection.primary_index()));
    doc.apply(&transaction, view.id);
    doc.append_changes_to_history(view);

    // after replace cursor may be out of bounds, do this to
    // make sure cursor is in view and update scroll as well
    view.ensure_cursor_in_view(doc, config.scrolloff);
}
