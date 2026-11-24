# Deleting a line closes its tab, once you confirm: here the tab you are in.
Output demo/close.svg

Source demo/settings.tape

Hide
Type "demo/session.sh 'old experiment'"
Enter
Sleep 3s
Show
Sleep 1s

Ctrl+B
Type "o"
Sleep 1.5s

Type "3j"
Sleep 400ms
Type "dd"
Sleep 700ms

Type@0ms ":wq"
Sleep 300ms
# The editor's write message and the popup closing are a few untidy
# frames; skip them.
Hide
Enter
Sleep 600ms
Show
Sleep 3s

# Closing stops the tab's processes, so it is confirmed first.
Type "y"
Sleep 3.5s
