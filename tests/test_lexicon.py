#!/usr/bin/env python3
"""One word, one meaning (the operator's decision of 2026-09-17), held by the text itself.

In Shaper OS the governor holds a SaaS ledger of desired state and a maker is the hand of one machine, one per
machine. PodMesh names its own roles otherwise: the PodMesh node (podmeshd on each host), the PodMesh manager
(the replicated coordination service) and the active manager (the replica holding the role at an epoch). This
test reads the tracked files where PodMesh speaks -- src/, tools/, tests/, packaging/, docs/, README.md,
INTENT.md, AGENTS.md -- and fails on "maker", "makers", "governor" or "governors" wherever ALLOWED does not
cover it: a previous name kept for compatibility for its stated time, a passage where the word names the
Shaper OS role, or this file. A word is a run of letters in any case; underscores, hyphens, dots, digits and
camelCase humps end it, so identifiers are read too, and "governs" or "governed" are other words.
No laboratory. Run: python3 -B tests/test_lexicon.py"""
import fnmatch, pathlib, re, subprocess, unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCOPE = ('src', 'tools', 'tests', 'packaging', 'docs', 'experiments', 'web', 'README.md', 'INTENT.md', 'AGENTS.md')
SELF = 'tests/test_lexicon.py'
WORDS = {'maker', 'makers', 'governor', 'governors'}
RULE = ('PodMesh names its own roles PodMesh node (podmeshd on a host), PodMesh manager (the replicated '
        'coordination service) or active manager (the replica holding the role at an epoch); "maker" and '
        '"governor" name Shaper OS roles only')

# (file pattern, phrase, reason). A phrase is a regular expression in which a space matches any run of
# whitespace, so that a rewrapped paragraph still matches; an occurrence is allowed when a match of an entry
# for its file contains it.
ALLOWED = [
    # (a) The previous name of the active manager's mark, kept until no running manager image reads it.
    ('packaging/podmesh-manager/universe/origin/server/index.mjs', r'/run/podmesh-manager/governor\.json',
     "the mark's previous path, read when the current one is absent"),
    ('packaging/podmesh-manager/universe/origin/tests/app.test.mjs', r'governor\.json',
     'the same previous path, in the test that holds the reading order'),
    ('packaging/podmesh-manager/universe/entrypoint.sh', r'/run/podmesh-manager/governor\.json',
     'the same previous path, removed with the current one at every start of the replica'),
    # (b) The fencing laboratory model and the integrated laboratory built on it: their own component is a
    #     Python class `Maker`, with the identifiers and the prose that name it.
    ('experiments/manager-fencing/*', r'[Mm]akers?|MAKER',
     "the fencing laboratory model's own class Maker, its derived identifiers and the prose describing that model"),
    ('experiments/manager-integrated/*', r'[Mm]akers?|MAKER',
     'the same model, composed with the epoch gate in the integrated laboratory'),
    ('experiments/manager-fencing/EVIDENCE.md', r'governor policy, makers',
     "the Shaper OS doctrine the evidence's scope line cites"),
    # (c) Shaper OS's own roles, where these documents describe the integration PodMesh serves.
    ('INTENT.md', r'\b[Gg]overnors?\b|\b[Mm]akers?\b',
     'the actors PodMesh serves: the governor holds desired state, one maker per host invokes PodMesh'),
    ('docs/README.md', r'\b[Gg]overnors?\b|\b[Mm]akers?\b|\bGOVERNOR\b|\bMAKER\b',
     "the index's Shaper OS sections: the SaaS governor, one maker per host, and the canon documents they cite"),
    ('docs/CONTROL-SERVICES-UNIVERSE.md', r'\b[Gg]overnors?\b|\b[Mm]akers?\b',
     'the SHAPER actors of this design, in its role table and its flows'),
    ('docs/MIGRATION-INTEGRATION.md', r'\b[Gg]overnors?\b|\b[Mm]akers?\b',
     'placement reported upward to Shaper OS, which PodMesh does not do yet'),
    ('docs/MIGRATION-PROTOCOL.md', r'\b[Gg]overnors?\b|\b[Mm]akers?\b',
     "the ShaperOS-mode requester: a maker acting on its governor's ledger row"),
    ('docs/RUST-IMPLEMENTATION-PLAN.md', r'\b[Gg]overnors?\b|\b[Mm]akers?\b',
     "the plan's Shaper integration slices"),
    ('docs/SHAPER-SUPERVISION.md', r'\b[Gg]overnors?\b|\b[Mm]akers?\b',
     "Shaper's supervision loop and the canon document it links"),
    ('docs/EXPERIMENTAL-SCOPE.md', r'\b[Gg]overnors?\b|\b[Mm]akers?\b',
     "Rule 37's governor and the SHAPER integration target's responsibilities"),
    ('docs/DELIVERY-CHECKLIST.md', r'\b[Gg]overnors?\b|\b[Mm]akers?\b',
     'the deliverables that keep the governor/maker separation and the fencing laboratory model'),
    ('docs/MANAGER-HA-ACCEPTANCE.md', r'\b[Gg]overnors?\b|\b[Mm]akers?\b',
     'HA-17, the end-to-end Shaper OS path a fictional SaaS governor drives'),
    ('docs/MANAGER-DEPLOYMENT.md', r'\b[Gg]overnors?\b|\b[Mm]akers?\b',
     'what the deployment refuses to assume: an alternative Governor or a Maker as activation authority'),
    ('docs/MANAGER-G2-DURABLE-EXCHANGE.md', r'\b[Gg]overnors?\b|\b[Mm]akers?\b',
     'the Shaper OS integration named as out of this increment (G7)'),
    ('docs/LOCAL-API.md', r"a maker forwarding a governor's row", 'who may ask for a reclaim in ShaperOS mode'),
    ('docs/REVIEW-EXPERIMENTAL3.md', r'\b[Gg]overnors?\b|\b[Mm]akers?\b',
     'the review naming the Shaper authority integration as pending'),
    ('docs/REVIEW-EXPERIMENTAL4.md', r'\b[Gg]overnors?\b|\b[Mm]akers?\b', 'the same, in the fourth review'),
    ('docs/REVIEW-MANAGER-PACKAGE-QUALIFICATION.md', r'\b[Gg]overnors?\b|\b[Mm]akers?\b',
     'the human and organizational roles the package keeps apart'),
    ('experiments/registry/README.md', r"[Tt]he governor's desired-state ledger",
     'the Shaper OS ledger the registry stays separate from'),
    ('web/tests/fractal-model.test.mjs', r"'governor'",
     "the Shaper OS SaaS governor as a relationship kind in the console's fractal fixture"),
    ('*', r'10_HELM_GOVERNOR_MAKER_MAPPING\.md', "the canon document's file name, cited as a link"),
    # A citation of the fencing laboratory's model, whose class for the node is `Maker`.
    ('*', r'\(the model' + chr(39) + r's "maker"\)', "the fencing laboratory model's own word for the node, quoted as such"),
]
COMPILED = [(pattern, re.compile(phrase.replace(' ', r'\s+'))) for pattern, phrase, _ in ALLOWED]


def tracked():
    out = subprocess.run(['git', 'ls-files', '-z', '--', *SCOPE], cwd=ROOT, capture_output=True)
    if out.returncode:
        raise RuntimeError(f'git ls-files failed in {ROOT}: {out.stderr.decode(errors="replace").strip()}')
    return sorted(p for p in out.stdout.decode().split('\0') if p and p != SELF)


def words(text):
    """Every word of a text with its offset: runs of letters, split again at camelCase humps."""
    for run in re.finditer(r'[A-Za-z]+', text):
        for part in re.finditer(r'[A-Z]?[a-z]+|[A-Z]+(?![a-z])', run.group()):
            yield run.start() + part.start(), part.group()


def scan():
    """The occurrences no allowance covers, and how many occurrences each allowance covers."""
    stray, used = [], [0] * len(ALLOWED)
    for path in tracked():
        file = ROOT / path
        if not file.is_file():
            continue
        data = file.read_bytes()
        if b'\0' in data:
            continue
        text = data.decode('utf-8', errors='replace')
        hits = [(offset, word) for offset, word in words(text) if word.lower() in WORDS]
        if not hits:
            continue
        spans = [(i, m.span()) for i, (pattern, regex) in enumerate(COMPILED) if fnmatch.fnmatchcase(path, pattern)
                 for m in regex.finditer(text)]
        lines = text.split('\n')
        for offset, word in hits:
            covering = {i for i, (start, end) in spans if start <= offset and offset + len(word) <= end}
            for i in covering:
                used[i] += 1
            if not covering:
                number = text.count('\n', 0, offset) + 1
                stray.append((path, number, word, lines[number - 1].strip()))
    return stray, used


class Lexicon(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.stray, cls.used = scan()

    def test_maker_and_governor_name_shaper_os_roles_only(self):
        if self.stray:
            self.fail(f'{len(self.stray)} use(s) of maker/governor outside the allow-list. {RULE} (the operator\'s '
                      'decision of 2026-09-17). Reword it; only a Shaper OS passage or an identifier kept for '
                      f'compatibility gets an entry, with its reason, in ALLOWED ({SELF}).\n'
                      + '\n'.join(f'  {path}:{number}: "{word}" -- PodMesh node / PodMesh manager / active manager: {line[:160]}'
                                  for path, number, word, line in self.stray))

    def test_every_allowance_still_covers_something(self):
        unused = [f'  {pattern}: {phrase} ({reason})' for (pattern, phrase, reason), n in zip(ALLOWED, self.used) if n == 0]
        if unused:
            self.fail('allowances that no longer cover anything; remove them from ALLOWED:\n' + '\n'.join(unused))


if __name__ == '__main__':
    unittest.main(verbosity=1)
