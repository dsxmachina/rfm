# Feature-Request: Async-Commands

Right now, there is some limited functionality of executing shell commands.
This is mostly hardwired - e.g. "zip / unzip" calls "tar" or "zip" under the hood to work with archives.

I would like to do two things here:
1. Expand this logic, so that the user can map arbitrary shell commands (which in some way work on the marked or selected elements),
   to some keybinding. This would enable a lot of flexibility for the user, which is not bound to what I can think of is useful.
2. I want those commands to be executed asynchronously on some "command-manager" on another thread.

This async approach would have the benefit, that if the command takes a long time to execute (e.g. unzipping a big archive),
the use can still use the file-manager, while the command executes in the background.
New commands should go into a queue (so there can only be one active shell command in the background) to avoid
weird synchronization issues.

We would require an indicator of how many calls are in the queue (with a small widget in the bottom right maybe?) and a progress-bar or
spinner, that tells us that there is still a command running.
