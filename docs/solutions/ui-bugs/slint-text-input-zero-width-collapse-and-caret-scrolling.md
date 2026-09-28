---
title: "Slint TextInput Zero-Width Collapse in HorizontalLayout and Auto-Scroll Caret Tracking"
date: "2026-09-25"
category: "ui-bugs"
module: "apps/gibberish-client"
problem_type: "ui_bug"
component: "frontend"
symptoms:
  - "User completely unable to click or focus the text input field in the Slint desktop client"
  - "No cursor appears and no characters can be typed into the chat entry box"
  - "Long messages overflowing the text input viewport become hidden without scrolling"
root_cause: "logic_error"
resolution_type: "code_fix"
severity: "high"
tags:
  - "slint"
  - "ui"
  - "text-input"
  - "horizontal-layout"
  - "focus-management"
  - "auto-scroll"
---

# Slint TextInput Zero-Width Collapse in HorizontalLayout and Auto-Scroll Caret Tracking

## Problem
In the Slint mesh desktop client (`apps/gibberish-client`), users were completely unable to enter text into the chat message entry box. Clicks anywhere inside the input box failed to focus or show a caret, preventing any message submission.

## Symptoms
- Clicking inside the message input box produced no visual feedback, no cursor caret, and accepted no keyboard keystrokes.
- The send button on the right was visible, but the input area appeared inert.
- When replacing `TextInput` with `std-widgets` `LineEdit`, the widget rendered with a hardcoded light-gray background (`#c0c0c0`) that severely clashed with the dark Tactical Obsidian theme.
- When typing long messages past the visual boundaries of a raw `TextInput`, newly entered characters overflowed invisibly outside the clipped container without auto-scrolling to follow the caret.

## What Didn't Work
1. **Binding `text: ""` directly**: An initial hypothesis suspected that `text: ""` declared inside a component instance created a continuous reactive binding that fought user input. While property bindings in Slint are reactive, literal string initializers do not continuously clear text.
2. **Replacing with `LineEdit` from `std-widgets.slint`**: While `LineEdit` automatically handled focus and click-to-caret positioning, it could not be themed cleanly in standard Slint 1.9 without a full native styling overhaul, leaving an obtrusive light-gray box in the dark interface. Furthermore, `forward-focus: chat_view` at `MainWindow` level failed compilation because composite components inheriting `Rectangle` are not recognized as focusable primitives by Slint's build macro.
3. **Leaving `alignment: space-between` on the container `HorizontalLayout`**: Even with `horizontal-stretch: 1`, Slint's `HorizontalLayout` disables automatic stretching when `alignment: space-between` is present, shrinking elements without fixed widths to their intrinsic size.

## Solution
1. **Removed `alignment: space-between`**: Removed the `space-between` alignment on the input's `HorizontalLayout` in [`apps/gibberish-client/ui/chat_view.slint`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-client/ui/chat_view.slint#L189), restoring `horizontal-stretch: 1` behavior so the input field expands to fill all available horizontal space up to the Send button.
2. **Built Themed `HackerInput` Architecture**: Wrapped `TextInput` in an outer `Rectangle` with `clip: true` and explicit dimensions (`width: 100%; height: 100%`), ensuring the click target matches the entire visual bounding box.
3. **Implemented Slint Caret Auto-Scroll**: Bound `x` to `computed-x` and attached a `cursor-position-changed(pos)` handler that slides the text horizontally to keep the caret visible as long messages are typed:

```slint
// Input Field Wrapper in chat_view.slint
Rectangle {
    horizontal-stretch: 1;
    clip: true;

    // Placeholder text (left-aligned, visible when empty and unfocused)
    if input_box.text == "" && !input_box.has-focus : Text {
        x: 0px;
        width: 100%;
        text: "Type a message... (Press Enter to send)";
        color: #64748b;
        font-size: 12px;
        font-family: "JetBrains Mono";
        horizontal-alignment: left;
        vertical-alignment: center;
    }

    input_box := TextInput {
        property <length> computed-x: 0px;

        x: min(0px, max(parent.width - self.width - self.text-cursor-width, self.computed-x));
        width: max(parent.width - self.text-cursor-width, self.preferred-width);
        height: 100%;
        color: #f1f5f9;
        font-size: 12px;
        font-family: "JetBrains Mono";
        single-line: true;
        horizontal-alignment: left;
        vertical-alignment: center;

        cursor-position-changed(pos) => {
            if pos.x + self.computed-x < 0px {
                self.computed-x = - pos.x;
            } else if pos.x + self.computed-x > parent.width - self.text-cursor-width {
                self.computed-x = parent.width - pos.x - self.text-cursor-width;
            }
        }

        accepted => {
            if (self.text != "") {
                root.send_message(self.text);
                self.text = "";
                self.computed-x = 0px;
            }
        }
    }
}
```

4. **Symmetrical Reset**: Updated both `accepted` (Enter key) and `send_touch.clicked` (Send button) to reset `input_box.text = ""` and `input_box.computed-x = 0px`, preventing subsequent messages from rendering at a shifted offset.
5. **Dynamic Focus Glow**: Bound the container's border to `input_box.has-focus ? #38bdf8 : #1e293b`, providing clear visual feedback when the field has keyboard focus.

## Why This Works
- **Slint Layout Stretching**: In Slint's layout model, `HorizontalLayout` with `alignment: space-between` sizes each child to its natural/preferred width and distributes excess space into the gaps. A `TextInput` containing `""` has a preferred width of 0 pixels, collapsing the mouse click hit-test geometry completely. Removing `alignment: space-between` enables the default stretch algorithm, causing `horizontal-stretch: 1` to allocate all remaining pane width to the input wrapper.
- **Caret Tracking Math**: The formula `min(0px, max(parent.width - self.width - self.text-cursor-width, self.computed-x))` clamps the horizontal offset so text slides left into the clipped container as typing progresses, while preventing overscroll past the start or end of the text.
- **Symmetrical Reset**: Clearing `computed-x = 0px` alongside `text = ""` on both submit pathways guarantees that short subsequent messages immediately re-align to `x = 0px`.

## Prevention
- **Avoid `alignment: space-between` with Stretched Inputs**: Never use `alignment: space-between` or explicit alignment modes on layouts containing children that rely on `horizontal-stretch: 1` without a defined `min-width`.
- **Verify Click Targets in Visual / Interactive Tests**: Test interactive components by verifying that input fields declare explicit widths (`width: 100%`) or `min-width` so they retain clickable hit-test areas even when initialized empty.
- **Ensure Symmetric State Resets**: When implementing custom viewport scrolling or caret tracking on text inputs, ensure all submission paths (Enter key press, Send button click, programmatic clears) reset both the string state and the scroll offset coordinate.

## Related Issues
- Commit [`f0ddbf2`](https://github.com/derpy4me/gibberish/commit/f0ddbf2) (Introduced `space-between` layout collapse)
- Commit [`1e56343`](https://github.com/derpy4me/gibberish/commit/1e56343) (Switched to LineEdit and identified styling/forward-focus limitations)
- Commit [`a34aacf`](https://github.com/derpy4me/gibberish/commit/a34aacf) (Final verified fix with HackerInput and auto-scroll caret tracking)
