//! A compact developer prompt. Lua output stays in the launch terminal.

use super::{State, lucide::X};
use crate::theme::{SURFACE_BORDER, SURFACE_OPAQUE};
use rune::{El, col, color::hex, row, text};
use rune_kit::{Button, ButtonVariant, IconButton, IconButtonVariant, Kit, TextInput};

fn submit(state: &mut State) {
    let source = state.lua_console_input.text.trim().to_owned();
    if source.is_empty() {
        return;
    }
    state.scripts.eval(source);
    state.lua_console_input.clear();
    state.status = "Lua command sent; inspect the launch terminal".into();
}

pub(crate) fn view(state: &State, kit: &Kit) -> El {
    let prompt = TextInput::new(&state.lua_console_input)
        .label("Lua command")
        .placeholder("veyra.benchmark()")
        .width(None)
        .on_edit(|s: &mut State| &mut s.lua_console_input)
        .on_submit(|s: &mut State, _| submit(s))
        .build(kit);
    col()
        .key("lua-console-overlay")
        .w(520.0)
        .pin(None, Some(82.0), None, Some(82.0))
        .px(18.0)
        .py(16.0)
        .gap(10.0)
        .radius(18.0)
        .fill(hex(SURFACE_OPAQUE).unwrap())
        .border(1.0, hex(SURFACE_BORDER).unwrap())
        .child(
            row()
                .w_full()
                .justify_between()
                .child(text("Lua console").font(kit.ty.ui).color(kit.c.text_base))
                .child(
                    IconButton::new(&X)
                        .label("Close Lua console")
                        .variant(IconButtonVariant::GhostMuted)
                        .on_press(|s: &mut State| s.lua_console_open = false)
                        .build(kit)
                        .radius(999.0),
                ),
        )
        .child(
            text("Enter a Lua expression or statement. Output and errors are printed to the terminal that launched Veyra.")
                .w_full()
                .wrap_anywhere()
                .font(kit.ty.small)
                .color(kit.c.text_faint),
        )
        .child(row().w_full().gap(8.0).child(prompt).child(
            Button::new("Run")
                .variant(ButtonVariant::Contrast)
                .on_press(submit)
                .build(kit),
        ))
        .child(
            text("Examples:  veyra.benchmark()    ·    veyra.log(veyra.get_statistics().render_fps)")
                .w_full()
                .wrap_anywhere()
                .font(kit.ty.small)
                .color(kit.c.text_faint),
        )
}
