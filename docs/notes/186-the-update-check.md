# 186. The update check (2026-09-27)

A tester who installed 0.1.1 has no way to learn that 0.1.2 is out
short of watching the repository, and a report against a fixed bug
costs both sides a round trip. So the editor asks, once a day at most,
whether a newer release exists, and says so in the left pane above
Settings... and Report a problem..., where it waits without
interrupting: the status line is transient, and a dialog at launch is
the wrong weight for news the user may not want today.

The source is GitHub's releases API,
`/repos/jessolmstead/greycard/releases/latest`, not a file we host.
The releases are already there, so the answer cannot drift from what
is actually published; `latest` leaves out drafts and releases marked
as pre-releases; and there is no server of ours to keep up, to log
requests, or to be asked about. The request goes over HTTPS only,
follows at most two redirects, and carries a User-Agent of
`greycard/<version>`, which GitHub requires of every client, and the
Accept header its API documents; with them go the Host header HTTP
itself needs, and nothing else: no token, no query, no compression
asked for. It goes through a proxy when the environment names one
(`HTTPS_PROXY`, `ALL_PROXY`), as ureq does by default. GitHub sees the
address it comes from, as it would for any page. The unauthenticated
limit (60 an hour an address) is far past one a day.

It is on by default, with a switch in the Settings sheet and, beside
it, one plain sentence saying what is sent. Off by default would
leave exactly the users who need it (testers on an old build, who
never open Settings) never told. That means the first launch sends
its request before the switch can have been seen, and that is the
decision: one GET of public information, carrying nothing about the
user, is the cost of the news reaching the people who need it. The
sentence is there so the user does not have to take "no telemetry"
on trust: the request is one anyone can make from a browser, and the
words say what it carries. Check now runs the same request from the
sheet and answers there: the latest, a version available, GitHub
answering without a release, or GitHub not reached. The button waits
for its answer, and an answer to an earlier press is dropped by its
number rather than said over a later one.

Nothing is downloaded or installed. The entry opens the release page
in the browser, through the same path Report a problem uses, and the
user decides. An updater would need signing, a way to replace a
running binary on three platforms, and trust in a download path we
would then own; the release page already has the notes, the
checksums and the builds, and a person reading it is the review. A
page the answer names outside the repository is not opened; the
repository's releases page stands in.

The throttle: the time of the last check that GitHub answered, and
the release it last named (tag and page), are kept in the settings.
Any HTTP answer counts, a rate limit's 403, a 404, a 5xx or a body
that is not a release, so a GitHub that keeps refusing is asked once
a day and not every launch; a refusal keeps the release known before.
Only a failure to reach GitHub at all (DNS, connection, timeout)
leaves the time as it was, so the next launch tries again, and a
release found before still shows offline. A launch within 24 hours of
the last answer asks nothing; a clock set back past it asks again. The
check runs on its own thread, is never joined, and times out in five
seconds, so a quit is never held by it; a snapshot, a screenshot, an
export and the tests never make it.

Versions compare as major.minor.patch in numbers (0.1.10 is after
0.1.9); a `-` suffix makes a pre-release, which ranks below its
release, and a `+` suffix is ignored. A release build is never offered
a pre-release, so an rc tag that slipped through `latest` reaches no
one on a release, while an rc build is offered its final. A tag that
does not read as a version is no news, never an error. The entry's x
remembers the tag it dismissed, so that release is not offered again
at launch and the next one is; a Check now that finds the dismissed
tag forgets the dismissal, since that is what it was asked about.

The entry costs the left pane one row at its foot, above Settings...
and Report a problem..., where it takes height from the scrolling
sections. The History box now keeps three rows (or its whole list, if
shorter) at least, so the pane scrolls before that list is squeezed
out, as it was at 950 px with the entry showing. The Settings sheet,
now taller than a short window, scrolls within the window less a
margin, with Done kept below the scroll.
