# Rules for hospital-demo

These apply to hospital-demo only, not every repo using this team.

An interval with no fixed period, such as `prn`, reads as zero duration (`ParseInterval` returns `0, nil`, zero meaning "no fixed interval"). It has no number of doses a day: return an error (`dose: "prn" has no doses a day`), never a number. This applies when reading a prescription form with no fixed interval; it does not decide `q0h`, which hospital-demo#1 addresses.

Source decision: https://github.com/sunfmin/hospital-demo/issues/3#issuecomment-6135545374

Retained from the example's former precedents file. When adopting this team for hospital-demo, incorporate this rule into that repo's own rules. Other repos retain their own interval semantics.
