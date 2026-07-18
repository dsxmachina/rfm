# RFM - New design

I want you to re-design the logic of how the rendering and event-handling is done (basically what the panel-manager is doing right now).
The current design has no clear separation of concerns, when it comes to event-handling and rendering logic.
The PanelManager does the event-handling, keeps track manually of which things require a redraw (using some booleans).
Another thing that is quite cumbersome is the "state" logic:
Different things should happen depending on the current state (e.g. if a keyboard input is fed into a modal, or a search bar,
or is interpreted as a command. This is *also* done by the panel-manager, which is why this thing is 1200 lines long.

## Our goal

The goal is to refactor the panel-manager and think of a design, that separates the event-handling + drawing + modes logic a little better.
Regarding drawing: The best thing would be, if the compositor could take care of all drawing (instead of the akward split between panels
and widgets) - but if this approach would cause too many downsides, its okay to not go down that route.
In the end: We want to have *less* complexity and not *more* complexity (e.g. represented in lines of code, or long, cumbersome functions/modules).

## What to keep

Things that we want to keep and not change:
    - the logic that we can pre-fetch panel content
    - the preview-manager and how it functions
    - the file-watchers that tell us if a panel has updated

## How to change

Use the ./agent directory to keep track of you plan and your progress.

Use Git to commit your changes in meaningful chunks

If something does not work correctly, you are allowed to use git to undo you "bad" commits.

When you are done, update the ARCHITECTURE.md file to contain a brief description of you new architecture,
design principles and the logic behind them.
