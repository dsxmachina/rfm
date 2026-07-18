# Feature-Request: Rate-Limiter

We use the preview-manager to generate previews of files and directories.

The communication is performed with a channel - if a panel requests a change, it can do so via a channel,
then the preview-manager will generate the preview and send the result back to the panel that requested this.
There is some logic involved to identify the panel for this, but in general this approach is rock-solid.

However; there are times when the file-watchers (which watch for changes in the current directory, and immediately fire
a "request-update" event for this panel to the preview-manager) generate a lot of preview-requests rather quickly e.g.
when copying or deleting a large number of files; unzipping an archive, or when downloading a lot of things.
This is a challange for the preview-manager, as it simply spawns a new task on every request (which slows down the machine),
even if we never really need this frequency of updates.

Currently, the way to prevent this is a method call "freeze", which freezes the content of a panel in place,
and making it not fire any update requests, by disabling the watchers. This itself causes some problems;
e.g. we might miss a critical notify-event and end up with a panel-state that is "too old" and outdated.

## Proposed solution

What we should do instead is, to never un-register the watchers (so remove the freeze logic entirely),
but instead use some really fancy rate-limiting on the preview-manager.

When e.g. 50 update request events fire within 1 second - we don't require 50 single updates of the panel, 
thus we don't require to respect all 50 events. Instead: We only need to respect 2 - the first one and the "last" one after 1 second.
Note: 1 second is an arbitrary number here, which should be configurable - let's call it X now.

So the rate-limiter should work in a way, that it allows an incoming requests **for the same panel AND the same directory** every X seconds.
If a request comes in and is rate-limited; it should not be dropped - but instead it should be delayed, such that it fires again X seconds
after the first event (that basically blocked this one). If there already is an event waiting at the next "timeslot" (first event + X seconds),
then the event should be dropped.
I don't know what this type of rate-limiting is called (or if this is still rate-limiting), but this is how I would like it.
Then if a watcher fires 100 events per second, it's okay, we will still only redraw the panel once every X seconds (e.g. X=0.5),
but at the same time, we keep the panel up-to-date all the time.
