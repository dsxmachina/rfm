# Features
- the history should be saved by the manager
- traversing left should also consider history, so we have no issue when we follow symlinks

- Support for multiple tabs
  - each tab with its own history and set of panels

- [x] Generate preview for whole directory content when idle

- [x] Imrove panel creation time

- [ ] Improve drawing speed

## Search

### General
- while typing: 
  - only show content where pattern matches
  - TAB cycles through matches
  - n-th match is selected

- when hitting enter:
  - everything is displayed normally
  - selection stays as is (so one occurence is selected)
  - everything where the pattern matches is marked

- search-and-replace on marked items:
  - when searching and some items are marked, add ability to replace substring with new input
  - example: search for "png" displays all ".png" images, and now we can type "jpeg" to replace
    every "png" with "jpeg" (which would be stupid, but just as an example)

- In general:
  - Searching and Marking items should be linked,
    because if we searched for an item, we propably want to execute some action with it
  - Every item that matches our search should automatically be marked
  - upon hitting enter, the whole content of the directory is shown again, but all matching items are marked
    -> we now can do stuff with them (e.g. Ctrl+C, Ctrl+X etc.)
  - n and N cycle through marked items in the current directory (vim-style)


### Fuzzy
- Seperate mode: Bottom panel pops up for fuzzy search, miller panels get reduced to the top half
- similar to "cd": When we type, we recursively search from the current directory, and the bottom panel shows the results
  ( like "skim" does basically)
- the search results should be selectable (like in skim)
- if you select a result, the panels automically adjust and jump into the directory (like our "cd" command)
- the best fitting match is automatically selected
- when you hit enter, you jump into the selected directory and the matching file or dir is already selected for further actions
