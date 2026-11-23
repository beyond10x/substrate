# R-02 decisions

## 1. A section bar on narrow screens

Finding: UXF-011. Target metric: actions from a deep section to the section list at 390x844, now
3, expected 1.

Question: below 997 px, should "On this page" stay under the header as a bar that shows the
current section and opens the section list?

Yes: this site carries its own copy of the shared Docs System's section-list component, with the
bar. Shot: none yet; it is drawn when the answer is yes.

No: the closed "On this page" disclosure at the top of the article stays, and the finding goes to
the Docs System as its own.

Default if unanswered by the next round: no, because the component belongs to the shared Docs
System and a copy in this repository would have to follow every change of the original.

Answer: no, 2026-11-22. The operator's pick, relayed by session `host-operator`: "No: Docs System
fixes it". This site keeps no copy of the component, and UXF-011 is filed with the Docs System.
