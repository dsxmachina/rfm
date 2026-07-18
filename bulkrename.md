# Feature-Request: Bulkrename

Right now, you can only rename a single file and that's it.

We want to expand this - such that the "rename" command can distinguish between renaming a single file (which works fine and nothing should be changed here),
or renaming multiple files. For this, instead of only working on the selected file, we can use the "marked_or_selected" logic, to differentiate between single-rename and bulkrename.

## Concrete implementation

Since bulkrenaming requires editing multiple names at once, this operation should be done in the $EDITOR of the user (e.g. vim or emacs).
Upon entering the "bulkrename" mode/logic, a tmp file should be created with one line per file that should be renamed (each line is the current filename),
and when this file is saved and the editor is closed, the renaming should happen - one line at a time.

This is an example of how this file could look like:
```
filename-1.md
filename-2.txt
asdf.pdf
random.txt
```

Now the user can edit every line - so it may look like this:
```
new-filename-1.md
new-filename-2.txt
random-asdf.pdf
asdf-random.txt
```

So `filename-1.md` will become `new-filename-1.md` and so on.

## Security checks

Before the actual renaming happens, we should check if the target name is already taken and if so, create a feedback loop:
Re-open the editor with the (new) filenames - add a comment (syntax ' #') behind every line that could not be renamed.

E.g.:
```
new-filename-1.md #< new-filename-1.md already exists, please choose a different name and delete this comment
new-filename-2.txt
random-asdf.pdf
asdf-random.txt
```

Nonetheless - even after the check, we should use our "smart-move-file" logic (that checks if the target does exist, and apends underscores to the filename,
until the file can be created). Because someone else could write to the disk at the time after we performed our check!
