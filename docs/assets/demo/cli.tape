# kurama-cli.gif: one session calling an API, a CSV and a database.
# Run from the repository root after docs/assets/demo/setup.sh.
Output "docs/assets/kurama-cli.gif"
Source "docs/assets/demo/settings.tape"
Set Height 640

Hide
Type "source /tmp/kurama-demo/env.sh; clear"
Enter
Sleep 500ms
Show

Type "kurama status"
Sleep 400ms
Enter
Sleep 2s

Type `kurama api placeholder users/get -P id=1 --jq '{name, email, company: .company.name}'`
Sleep 400ms
Enter
Sleep 2500ms

Type `kurama data orders.csv --query 'SELECT status,count(*) AS n, sum(amount) AS yen FROM data GROUP BY 1' | column -t`
Sleep 400ms
Enter
Sleep 3s

Type `kurama db app --preview orders --max-rows 3 | column -t`
Sleep 400ms
Enter
Sleep 3s
