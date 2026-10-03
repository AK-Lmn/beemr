#!/bin/bash
# Prepares the two-pane demo session, then pastes Alice's real ticket into
# Sara's pane once it appears (as a user would after copying it).
D=/tmp/beemr-demo
tmux kill-server 2>/dev/null
rm -f $D/alice.log
rm -rf $D/sara/vacation-photos*
tmux -f /dev/null new-session -d -s demo -x 176 -y 34 -c $D/alice
tmux set -g status off
tmux set -g pane-border-status top
tmux set -g pane-border-format ' #{pane_title} '
tmux set -g pane-border-style fg=colour240
tmux set -g pane-active-border-style fg=colour33
tmux split-window -h -t demo -c $D/sara
tmux select-pane -t demo.0 -T "Alice's laptop"
tmux select-pane -t demo.1 -T "Sara's PC (anywhere else)"
tmux send-keys -t demo.0 "export PATH=$D/bin:\$PATH BEEMR_HOME=$D/alice/config PS1='\$ ' && clear" Enter
tmux send-keys -t demo.1 "export PATH=$D/bin:\$PATH BEEMR_HOME=$D/sara/config PS1='\$ ' && clear" Enter
tmux pipe-pane -t demo.0 "cat >> $D/alice.log"
tmux select-pane -t demo.0
nohup bash -c '
  for i in $(seq 1 120); do
    t=$(grep -o "beemr get [A-Za-z0-9_-]*" /tmp/beemr-demo/alice.log 2>/dev/null | tail -1 | cut -d" " -f3)
    [ -n "$t" ] && break; sleep 0.25
  done
  sleep 7
  tmux send-keys -t demo.1 "beemr get $t"
' >/dev/null 2>&1 &
