# Some exceptions we need to handle:

- [ ] if we followed a symlink, we have to remember the parent of the symlink,
      so if we try to walk "left" again, we don't end up in an unexpected directory
