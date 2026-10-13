# The team

The roles this team names, each by the short name its workflows call it. A role is a directory — a
whole repo, or a subdirectory of one; its rules, skills and hooks are in it, and nothing of them is
here.

```yaml
roles:
  {role}: {repo}
```

Every key of this file, and of a workflow and a patrol, is in `afkconvoy --help`. After each edit
run `afkconvoy check --dir .` here, until it exits 0.
