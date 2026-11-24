# Adding a line adds a tab.
Output demo/create.svg

Source demo/settings.tape

Hide
Type "demo/session.sh tests"
Enter
Sleep 3s
Show
Sleep 1s

Ctrl+B
Type "o"
Sleep 1.5s

# A new tab after api's last one.
Type "7j"
Sleep 400ms
Type@0ms "o- review PR 42"
Sleep 300ms
Escape
Sleep 700ms

Type@0ms ":wq"
Sleep 300ms
# The editor's write message and the popup closing are a few untidy
# frames; skip them.
Hide
Enter
Sleep 600ms
Show
Sleep 4s
