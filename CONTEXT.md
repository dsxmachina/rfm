# rfm

A terminal file manager presenting the filesystem as Miller columns, driven
entirely by keyboard input.

## Language

### Layout

**Panel**:
One of the three Miller columns: left (parent directory), center (current
directory), right (preview of the selection).
_Avoid_: pane, window, view

**Preview**:
The right panel's rendering of the current selection — directory listing,
text excerpt, or image.

**Selection**:
The single entry the cursor is on in the center panel.
_Avoid_: cursor, highlight

**Marking**:
Flagging multiple entries for a batch operation (cut, copy, bulkrename).
Distinct from the selection.
_Avoid_: selecting multiple

### Input

**Mode**:
The interpretation currently applied to keyboard input. Exactly one is
active: Normal, or one modal mode.

**Modal mode**:
A mode that captures all key input for its own dialog (search, rename,
create-item, the consoles) until it exits back to Normal.
_Avoid_: dialog, popup, overlay

**ModalInput**:
The interface every modal mode implements: keys go in, mode operations come
out, and the modal draws itself in its assigned region.

**Mode operation (ModeOp)**:
The effect a modal mode requests from the application (change directory,
update search, rename, exit, …). Modal modes request; the application
applies.
_Avoid_: command (taken by user-defined shell commands), action

**Modal region (ModalRegion)**:
The screen area a modal mode is granted for drawing: the console overlay or
the footer line. Assigning regions is layout's decision, not the mode's.

**Console**:
A modal mode with a centered overlay for navigating somewhere: DirConsole
(cd with completion) and Zoxide (frecency jump).

### Background work

**Queued command**:
A user-defined shell command executed sequentially in the background by the
command executor. Paths interpolated into it must be shell-escaped.

**Panel update**:
An asynchronously produced panel content delivery; may arrive out of order
and must supersede the currently shown state to be applied.
