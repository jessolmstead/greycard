#!/usr/bin/env python3
"""Reverse the split: inline every panel component instance in the
branch's app.slint back into App, then write the result so it can be
diffed (whitespace-stripped) against master's app.slint.

Also reports, for each instance, every binding that is not an
identity forward (x: root.x / x <=> root.x / cb(a) => { root.cb(a); }),
and for each forward, the section's declaration vs App's.
"""
import re, sys, os, json

UI = sys.argv[1]
OUT = sys.argv[2]


def strip_comments_mask(s):
    """Return a copy of s with comments and string contents replaced by
    spaces (same length), so brace matching ignores them."""
    out = list(s)
    i, n = 0, len(s)
    while i < n:
        c = s[i]
        if s.startswith('//', i):
            j = s.find('\n', i)
            j = n if j < 0 else j
            for k in range(i, j):
                out[k] = ' '
            i = j
        elif s.startswith('/*', i):
            j = s.find('*/', i) + 2
            for k in range(i, j):
                if out[k] != '\n':
                    out[k] = ' '
            i = j
        elif c == '"':
            j = i + 1
            depth = 0
            while j < n:
                if s[j] == '\\':
                    j += 2
                    continue
                if s[j] == '{':
                    depth += 1
                if s[j] == '}':
                    depth -= 1
                if s[j] == '"' and depth == 0:
                    break
                j += 1
            for k in range(i + 1, j):
                out[k] = ' '
            i = j + 1
        else:
            i += 1
    return ''.join(out)


def match_brace(m, i):
    assert m[i] == '{'
    d = 0
    for j in range(i, len(m)):
        if m[j] == '{':
            d += 1
        elif m[j] == '}':
            d -= 1
            if d == 0:
                return j
    raise ValueError


def top_statements(body, mbody):
    """Split a block body into top-level statements (text spans).
    A statement ends at ';' at depth 0 or at a '}' closing depth 1."""
    stmts = []
    i, n = 0, len(body)
    start = 0
    d = 0
    paren = 0
    while i < n:
        c = mbody[i]
        if c == '(' or c == '[':
            paren += 1
        elif c == ')' or c == ']':
            paren -= 1
        elif c == '{':
            d += 1
        elif c == '}':
            d -= 1
            if d == 0 and paren == 0:
                # a block statement ends here unless followed by ';'
                stmts.append(body[start:i + 1])
                start = i + 1
        elif c == ';' and d == 0 and paren == 0:
            stmts.append(body[start:i + 1])
            start = i + 1
        i += 1
    rest = body[start:]
    if rest.strip():
        stmts.append(rest)
    return stmts


def parse_components(path):
    s = open(path).read()
    m = strip_comments_mask(s)
    comps = {}
    for mo in re.finditer(r'(export\s+)?component\s+([\w-]+)\s+inherits\s+([\w-]+)\s*\{', m):
        o = mo.end() - 1
        c = match_brace(m, o)
        comps[mo.group(2)] = dict(base=mo.group(3), body=s[o + 1:c], mbody=m[o + 1:c], file=path)
    return comps


DECL = re.compile(r'^\s*(?:///[^\n]*\n\s*|//[^\n]*\n\s*)*(?:(in|out|in-out|private)\s+)?property\s*<[^>]*>+\s*([\w-]+)|^\s*(?:///[^\n]*\n\s*|//[^\n]*\n\s*)*(pure\s+)?callback\s+([\w-]+)|^\s*(?:///[^\n]*\n\s*|//[^\n]*\n\s*)*(public\s+)?(pure\s+)?function\s+([\w-]+)', re.S)


def is_decl(stmt):
    t = re.sub(r'(?m)^\s*//.*\n', '', stmt).strip()
    return bool(re.match(r'((in|out|in-out|private)\s+)?property\s*<|(pure\s+)?callback\s|(public\s+)?(pure\s+)?function\s', t))


comps = {}
for f in sorted(x for x in os.listdir(os.path.join(UI, 'panel')) if x != 'filter.slint'):
    comps.update(parse_components(os.path.join(UI, 'panel', f)))

app = open(os.path.join(UI, 'app.slint')).read()
report = []


def reconstruct(text):
    """Replace every instance of a panel component in text by its
    inlined body; recursive for nested panel components."""
    m = strip_comments_mask(text)
    names = '|'.join(sorted(comps, key=len, reverse=True))
    pat = re.compile(r'(?<![\w-])(%s)\s*\{' % names)
    out = []
    pos = 0
    for mo in pat.finditer(m):
        if mo.start() < pos:
            continue
        name = mo.group(1)
        # skip the component definitions themselves
        before = m[max(0, mo.start() - 40):mo.start()]
        if re.search(r'(component|inherits)\s+$', before):
            continue
        o = mo.end() - 1
        c = match_brace(m, o)
        inst_body = text[o + 1:c]
        inst_m = m[o + 1:c]
        comp = comps[name]
        stmts = top_statements(inst_body, inst_m)
        other = []
        forwards = []
        for st in stmts:
            t = st.strip()
            t1 = re.sub(r'(?m)^\s*//.*$', '', t).strip()
            if not t1:
                other.append(st)
                continue
            f1 = re.match(r'^([\w-]+)\s*(:|<=>)\s*root\.([\w-]+)\s*;$', t1)
            f2 = re.match(r'^([\w-]+)\s*(\(([^)]*)\))?\s*=>\s*\{\s*(root|keys)\.([\w-]+)\(([^)]*)\);\s*\}$', t1)
            if f1:
                forwards.append(('prop', f1.group(1), f1.group(2), f1.group(3)))
                if f1.group(1) != f1.group(3):
                    report.append(f'NONIDENT {name}: {t1}')
            elif f2:
                forwards.append(('cb', f2.group(1), f2.group(3), f2.group(4) + '.' + f2.group(5), f2.group(6)))
                if not (f2.group(4) == 'root' and f2.group(1) == f2.group(5) and (f2.group(3) or '') == f2.group(6)) and not (f2.group(1) == 'focus-keys' and t1.endswith('{ keys.focus(); }')):
                    report.append(f'NONIDENT-CB {name}: {t1}')
            else:
                other.append(st)
                report.append(f'OTHER {name}: {t1[:200]}')
        cstmts = top_statements(comp['body'], comp['mbody'])
        cbody = [st for st in cstmts if not is_decl(st)]
        cdecl = [st for st in cstmts if is_decl(st)]
        report.append(f'INSTANCE {name} fwd={len(forwards)} decls={len(cdecl)}')
        inner = ''.join(other) + ''.join(cbody)
        inner = reconstruct(inner)
        out.append(text[pos:mo.start()])
        out.append(comp['base'] + ' {' + inner + '\n}')
        pos = c + 1
    out.append(text[pos:])
    return ''.join(out)


res = reconstruct(app)
# the listed exceptions, undone
res = res.replace('root.focus-keys()', 'keys.focus()')
res = res.replace('@image-url("../icons/', '@image-url("icons/')
open(OUT, 'w').write(res)
open(OUT + '.report', 'w').write('\n'.join(report) + '\n')

# ---- declaration checks
def decls_of(body, mbody):
    d = {}
    for st in top_statements(body, mbody):
        if not is_decl(st):
            continue
        t = re.sub(r'(?m)^\s*//.*\n', '', st).strip()
        mo = re.match(r'((in|out|in-out|private)\s+)?property\s*<(.*?)>\s*([\w-]+)\s*(.*)$', t, re.S)
        if mo:
            d[mo.group(4)] = ('prop', mo.group(2) or 'private', mo.group(3), mo.group(5).strip())
            continue
        mo = re.match(r'(pure\s+)?callback\s+([\w-]+)\s*(.*)$', t, re.S)
        if mo:
            d[mo.group(2)] = ('cb', 'pure' if mo.group(1) else '', mo.group(3).strip(), '')
            continue
        mo = re.match(r'(public\s+)?(pure\s+)?function\s+([\w-]+)', t)
        d[mo.group(3)] = ('fn', mo.group(2) or '', '', '')
    return d

am = strip_comments_mask(app)
ao = am.index('export component App inherits Window {') + len('export component App inherits Window {') - 1
ac = match_brace(am, ao)
appdecl = decls_of(app[ao+1:ac], am[ao+1:ac])
ms = open(sys.argv[3]).read(); mm = strip_comments_mask(ms)
mo_ = mm.index('export component App inherits Window {') + len('export component App inherits Window {') - 1
mc = match_brace(mm, mo_)
masterdecl = decls_of(ms[mo_+1:mc], mm[mo_+1:mc])
print('App decls branch', len(appdecl), 'master', len(masterdecl))
for k in masterdecl:
    if k not in appdecl: print('  moved out of App:', k, masterdecl[k][:2])
for k in appdecl:
    if k not in masterdecl: print('  new in App:', k)
    elif appdecl[k] != masterdecl[k]: print('  CHANGED in App:', k, masterdecl[k], appdecl[k])

# instance forwards vs declarations
m = am
names = '|'.join(sorted(comps, key=len, reverse=True))
for mo in re.finditer(r'(?<![\w-])(%s)\s*\{' % names, m):
    name = mo.group(1); o = mo.end()-1; c = match_brace(m, o)
    cd = decls_of(comps[name]['body'], comps[name]['mbody'])
    bound = set()
    for st in top_statements(app[o+1:c], m[o+1:c]):
        t1 = re.sub(r'(?m)^\s*//.*$', '', st).strip()
        f1 = re.match(r'^([\w-]+)\s*(:|<=>)\s*root\.([\w-]+)\s*;$', t1)
        f2 = re.match(r'^([\w-]+)\s*(\(([^)]*)\))?\s*=>', t1)
        if f1:
            n, kind = f1.group(1), f1.group(2); bound.add(n)
            dd = cd.get(n); ad = appdecl.get(n)
            if not dd: print(f'{name}: {n} bound but not declared'); continue
            if dd[3] and dd[3] != ';': print(f'{name}: {n} has a default in the section: {dd[3]}')
            if kind == ':' and dd[1] != 'in': print(f'{name}: {n} one-way but section declares {dd[1]}')
            if kind == '<=>' and dd[1] != 'in-out': print(f'{name}: {n} two-way but section declares {dd[1]}')
            if ad is None: print(f'{name}: {n} not in App'); continue
            if dd[2].replace(' ','') != ad[2].replace(' ',''): print(f'{name}: {n} type {dd[2]} vs App {ad[2]}')
            if kind == '<=>' and ad[1] not in ('in-out','private'): print(f'{name}: {n} two-way on App {ad[1]}')
            if kind == ':' and ad[1] == 'in-out': print(f'NOTE {name}: {n} one-way from App in-out')
        elif f2:
            bound.add(f2.group(1))
            dd = cd.get(f2.group(1))
            if not dd: print(f'{name}: cb {f2.group(1)} not declared')
            elif dd[0] != 'cb': print(f'{name}: {f2.group(1)} is {dd}')
    for n, dd in cd.items():
        if n not in bound and dd[1] not in ('private', '') and dd[0] == 'prop':
            print(f'{name}: {n} declared {dd[1]} but not bound (default {dd[3]!r})')
        if n not in bound and dd[0] == 'cb':
            print(f'{name}: callback {n} declared but not forwarded')
        if dd[0]=='prop' and dd[1]=='private' or dd[0]=='fn':
            print(f'{name}: private {dd}  {n}')
