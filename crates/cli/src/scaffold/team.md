# The team

The roles this team names, each by the short name its workflows call it. A role is a directory — a
whole repo, or a subdirectory of one; its rules, skills and hooks are in it, and nothing of them is
here. One named `./path` is a directory of this team's own repo, its path written from that repo's
root.

```yaml
roles:
{roles}
```

Every key of this file is in `afkconvoy help team`, and a workflow's and a patrol's in
`afkconvoy help workflows` and `afkconvoy help patrols`. After each edit run `afkconvoy check {from}` here, until it exits 0. A role is read from GitHub, at the
head of its repo's default branch: an edit to one counts once it is pushed.
