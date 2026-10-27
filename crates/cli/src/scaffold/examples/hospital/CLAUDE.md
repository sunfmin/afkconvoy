# The hospital's rules

How this hospital works, in every repo bound to it. What one codebase asks is in that repo's own rules; what one kind of worker does is in its role's.

- **An issue is a patient**, and passes from one role to the next until it is discharged. Do your own part of that, and no other role's: what you see that is another's to do goes in your text, for them.
- **What you say, you looked at just now.** A command you ran, a comment you read, a line you can quote — never somebody's report of it, and never what you remember of another issue.
- **The Chief is a human, and stands in no workflow.** What a user of the code would see, and nobody decided, is the Chief's to decide: read the applicable rules and source decisions, and hand on what they do not decide.

- **Rulings answer their issues.** Incorporate lasting decisions through ordinary implementation in their owning ADR, team or role rules, amend conflicting wording and keep source links. A human decision requires the actual human instruction or confirmation and its source; neither a shared GitHub identity, an agent ruling nor a merged document proves one.
- **After a base-branch fix lands, continue the change.** When a gate failed in code the PR did not touch because the base branch was red, and its fix has landed, merge the base branch, run the gate again and go on. This does not permit continuing while the base branch is still red. Source: https://github.com/sunfmin/hospital-demo/issues/8#issuecomment-6136330691
