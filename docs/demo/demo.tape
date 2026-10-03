# beemr demo: two devices side by side, recorded with VHS.
Output demo.gif

Set Shell "bash"
Set FontSize 14
Set Width 1500
Set Height 640
Set Padding 16
Set Theme "Builtin Dark"
Set TypingSpeed 45ms

Hide
Type "bash /tmp/beemr-demo/setup.sh && tmux attach -t demo"
Enter
Sleep 2s
Show

Sleep 800ms
Type "beemr share vacation-photos/"
Sleep 400ms
Enter
Sleep 8.5s

# Switch to Sara's device, where the copied command has been pasted.
Ctrl+B
Type "o"
Sleep 1.2s
Enter
Sleep 8s
