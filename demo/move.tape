# Moving the tab you're in to another workspace: the split and the running
# server go with it.
Output demo/move.svg

Source demo/settings.tape

Hide
Type "demo/session.sh"
Enter
Sleep 3s
Show
Sleep 1s

# Open the outline.
Ctrl+B
Type "o"
Sleep 1.5s

# Cut `server` from notes, paste it at the end of api.
Type "j"
Sleep 400ms
Type "dd"
Sleep 700ms
# Down to api's last tab.
Type "5j"
Sleep 500ms
Type "p"
Sleep 1s
Type@0ms ":wq"
Sleep 300ms
# The editor's write message and the popup closing are a few untidy
# frames; skip them.
Hide
Enter
Sleep 600ms
Show

# Focus follows the tab to api: still split, still running.
Sleep 5s
