# The camera match's sample-size sweep, its tables: run the ignored
# test `camera_match_sample_size_sweep` in greycard-ui first (its doc
# says what it reads), then this with the sweep's work folder:
#
#     python3 tools/camera-match/sweep_tables.py <GREYCARD_SWEEP>
#
# Per set, size and pick: the fitted error, the error on the held-out
# pool, the sheet's (every frame held out of its own fit), the earlier
# four-frame figure and the seconds; the random picks as the mean of
# their seeds with the range. Then coverage at N against random at 2N,
# the pool by lens, how much of the lattice each pick reaches, and
# where each coverage pick's gain per frame falls under a threshold.
import csv, collections, glob, os, statistics, sys
S = sys.argv[1]
rows = list(csv.DictReader(open(S + '/results.csv')))
by = collections.defaultdict(list)
for r in rows:
    by[(r['set'], r['method'], int(r['n']))].append(r)
sets = list(dict.fromkeys(r['set'] for r in rows))
methods = ['random', 'sampler', 'cover', 'cover-even']
cols = ['fitted', 'held_pool', 'held_every', 'held_four']
out = []
for s in sets:
    out.append(f'\n### {s}\n')
    out.append('| N | pick | kept | fitted | held out, pool | held out, every frame | held out, four | seconds |')
    out.append('|---|---|---|---|---|---|---|---|')
    sizes = sorted({n for (ss, m, n) in by if ss == s})
    for n in sizes:
        for m in methods:
            rs = by.get((s, m, n))
            if not rs:
                continue
            def f(c):
                v = [float(r[c]) for r in rs]
                if len(v) == 1:
                    return f'{v[0]:.4f}'
                return f'{statistics.mean(v):.4f} ({min(v):.4f}-{max(v):.4f})'
            kept = '/'.join(r['kept'] for r in rs)
            secs = statistics.mean(float(r['total_s']) for r in rs)
            out.append(f'| {n} | {m} | {kept} | ' + ' | '.join(f(c) for c in cols) + f' | {secs:.0f} |')
print('\n'.join(out))

# The decision: coverage at N against random at 2N, on the pool.
print('\n### coverage at N against random at 2N (pool held out; every-frame in brackets)\n')
for s in sets:
    for n in (10, 20, 40):
        rnd = by.get((s, 'random', 2 * n))
        for m in ('cover', 'cover-even', 'sampler'):
            c = by.get((s, m, n))
            if not c or not rnd:
                continue
            rp = statistics.mean(float(r['held_pool']) for r in rnd)
            re = statistics.mean(float(r['held_every']) for r in rnd)
            print(f'{s} {m}@{n} pool {float(c[0]["held_pool"]):.4f} vs random@{2*n} {rp:.4f}: '
                  f'{float(c[0]["held_pool"]) - rp:+.4f} '
                  f'[every {float(c[0]["held_every"]):.4f} vs {re:.4f}]')

# The pool by lens, for the GFX.
pool = list(csv.DictReader(open(S + '/pool.csv')))
print('\n### the pool by lens\n')
agg = collections.defaultdict(list)
for r in pool:
    agg[(r['set'], r['method'], int(r['n']), r['lens'])].append(float(r['delta_e']))
for k in sorted(agg):
    if len({kk[3] for kk in agg if kk[0] == k[0]}) > 1:
        v = agg[k]
        print(k, f'{statistics.mean(v):.4f}', len(v))

print('reach (asked-about weights / even weights), mean over seeds')
for k in sorted(by):
    rs = by[k]
    print(k, f"{statistics.mean(float(r['reach']) for r in rs):.3f} / {statistics.mean(float(r['reach_even']) for r in rs):.3f}")
print()
for f in sorted(glob.glob(S + '/coverage-*.csv')):
    g = list(csv.DictReader(open(f)))
    first = {}
    for t in (1.0, 0.5, 0.25, 0.1):
        for r in g:
            if float(r['gain_pct']) < t:
                first[t] = (int(r['rank']), float(r['cumulative_pct']))
                break
    at = {n: float(g[n - 1]['cumulative_pct']) for n in (10, 20, 40) if len(g) >= n}
    forced = [int(r['rank']) for r in g if r['forced'] == 'true']
    print(os.path.basename(f), 'first rank under x% (rank, cumulative):', first, 'cumulative at 10/20/40:', at, 'forced ranks', forced)
