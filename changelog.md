# 0.3.2

- fix deadlock when resizing the window
- limit number of worker threads in tokio runtime to reduce number of spawned rfm processes

# 0.3.1

- greatly reduce binary size (see [here](https://github.com/dsxmachina/rfm/issues/5))
- implement zoxide mode

You can now change directories using zoxide. This option is now available in the `keys.toml` config:
```toml
[manipulation]
zoxide_query = [ "CD", "Cd", "cD" ] # "shift+cd" with mistakes
```

# 0.3.0

- fix incorrect display width of exotic unicode characters
- add general config-file `config.toml`
- move color config to general config
- add input parameter for startup path
