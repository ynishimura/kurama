# kurama-tui.gif and kurama-home.png / kurama-db.png: the home screen's
# tabs, the command palette, and the database explorer it opens.
# Run from the repository root after docs/assets/demo/setup.sh.
Output "docs/assets/kurama-tui.gif"
Source "docs/assets/demo/settings.tape"

Hide
Type "source /tmp/kurama-demo/env.sh; clear"
Enter
Sleep 500ms
Show

Type "kurama"
Enter
Sleep 2s
Down
Sleep 800ms
Screenshot "docs/assets/kurama-home.png"
Down 2
Sleep 800ms
Tab
Sleep 1s
Tab
Sleep 1s
Tab
Sleep 1s
Tab
Sleep 1s

Ctrl+K
Sleep 800ms
Type "app"
Sleep 800ms
Enter
Sleep 2s
Down
Sleep 400ms
Enter
Sleep 1500ms
Type "p"
Sleep 2s
Screenshot "docs/assets/kurama-db.png"
Type "s"
Sleep 500ms
Type "SELECT status, count(*) AS orders, sum(amount) AS total FROM orders GROUP BY status"
Sleep 600ms
Enter
Sleep 2500ms
Type "q"
Sleep 1s
Type "q"
Sleep 500ms
