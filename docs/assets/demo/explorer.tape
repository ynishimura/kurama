# kurama-explorer.gif and kurama-explorer.png: an OpenAPI description
# browsed, then one operation filled in, sent and filtered with jq.
# Run from the repository root after docs/assets/demo/setup.sh.
Output "docs/assets/kurama-explorer.gif"
Source "docs/assets/demo/settings.tape"

Hide
Type "source /tmp/kurama-demo/env.sh; clear"
Enter
Sleep 500ms
Show

Type "kurama api petstore"
Enter
Sleep 3s
Down 4
Sleep 1s
Screenshot "docs/assets/kurama-explorer.png"
Down 4
Sleep 1s
Type "/"
Sleep 300ms
Type "user"
Sleep 1s
Escape
Sleep 400ms
Type "q"
Sleep 800ms

Type "kurama api placeholder"
Enter
Sleep 2s
Down
Sleep 800ms
Enter
Sleep 800ms
Type "1"
Sleep 800ms
Enter
Sleep 3s
Type "j"
Sleep 600ms
Type ".company"
Sleep 2s
Enter
Sleep 2s
Type "q"
Sleep 500ms
