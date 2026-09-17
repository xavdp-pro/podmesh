#!/usr/bin/env python3
"""One word, one meaning (the operator's decision of 2026-09-17), held by the text itself.

In Shaper OS the governor holds a SaaS ledger of desired state and a maker is the hand of one machine, one per
machine. PodMesh names its own roles otherwise: the PodMesh node (podmeshd on each host), the PodMesh manager
(the replicated coordination service) and the active manager (the replica holding the role at an epoch). This
test reads the tracked files where PodMesh speaks -- src/, tools/, tests/, packaging/, docs/, README.md,
INTENT.md, AGENTS.md -- and fails on "maker", "makers", "governor" or "governors" wherever ALLOWED does not
cover it: an identifier kept for compatibility until its compatible rename, a passage where the word names the
Shaper OS role, or this file. A word is a run of letters in any case; underscores, hyphens, dots, digits and
camelCase humps end it, so identifiers are read too, and "governs" or "governed" are other words.
No laboratory. Run: python3 -B tests/test_lexicon.py"""
import fnmatch, pathlib, re, subprocess, unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCOPE = ('src', 'tools', 'tests', 'packaging', 'docs', 'README.md', 'INTENT.md', 'AGENTS.md')
SELF = 'tests/test_lexicon.py'
WORDS = {'maker', 'makers', 'governor', 'governors'}
RULE = ('PodMesh names its own roles PodMesh node (podmeshd on a host), PodMesh manager (the replicated '
        'coordination service) or active manager (the replica holding the role at an epoch); "maker" and '
        '"governor" name Shaper OS roles only')

# (file pattern, phrase, reason). A phrase is a regular expression in which a space matches any run of
# whitespace, so that a rewrapped paragraph still matches; an occurrence is allowed when a match of an entry
# for its file contains it.
ALLOWED = [
    # (a) Identifiers kept for compatibility: journals, APIs, stored reports and running manager images use
    #     them; a compatible rename at the manager image roll removes these entries.
    ('*', r'\bgovernor_mark\b', "the mark's effect kind in the network effects ledger and publisher_status's field"),
    ('src/publisher.rs', r'/run/podmesh-manager/governor\.json', "the mark's path, read by the origin in running manager images"),
    ('tools/roll-manager-image.py', r'\bPODMESH_GOVERNOR_ALIAS\b', 'the former name of the environment variable, still read'),
    ('tools/arm-publisher-follow.py', r"'governor': G,", "a key of the state the tool prints and stores (state.json, refresh.json)"),
    ('*', r'\bcheck-manager-governor-managed\.py\b', "a suite's file name, cited by other suites and documents"),
    ('tests/check-manager-governor-managed.py', r"'disposable-lab-m-u2-governor'", "the authorization reference in the hosts' journals and the HA ledger"),
    # (b) The Shaper OS governor and maker, where PodMesh speaks about its integration with Shaper OS.
    ('INTENT.md', r'The governor maintains desired state|One maker per host|become a second governor|'
                  r'fictional SaaS governor|Makers materialize them through PodMesh',
     'the actors PodMesh serves, in the integration paragraph and the initial proof'),
    ('docs/CONTROL-SERVICES-UNIVERSE.md', r'\b(?:Governor|Makers?)\b',
     'the SHAPER actors, capitalized as this document names them in its role table and flows'),
    ('docs/EXPERIMENTAL-SCOPE.md', r'not a SHAPER governor|the governor maintains declared desired state|'
                                   r'makers retrieve authorized work|the governor/maker separation',
     "Rule 37's governor and the SHAPER integration target's responsibilities"),
    ('docs/SHAPER-SUPERVISION.md', r'10_HELM_GOVERNOR_MAKER_MAPPING\.md|another governor or supervisor hierarchy|'
                                   r'The maker invokes permitted PodMesh operations|the governor retains desired state',
     "Shaper's supervision loop and the canon document it links"),
    ('docs/BACKUP-SERVER.md', r'\bMaker\b', 'the ShaperOS Maker, capitalized as this design names that organ above PodMesh'),
    ('docs/MIGRATION-INTEGRATION.md', r"the maker/governor|(?:a|no) maker or governor|a maker acting on a governor's row",
     'placement reported upward to Shaper OS, which PodMesh does not do yet'),
    ('docs/MIGRATION-PROTOCOL.md', r"the maker acting on its governor's ledger row|Requester \(tandem or maker\)|Transport by makers",
     "the ShaperOS-mode requester: a maker acting on its governor's ledger row"),
    ('docs/RUST-IMPLEMENTATION-PLAN.md', r'[Mm]aker integration|existing governor desired-state ledger|maker invokes PodMesh|'
                                         r'Governor-to-maker-to-PodMesh|alternative governor inside podmeshd|through a maker\.',
     "the plan's Shaper integration slices"),
    ('docs/LOCAL-API.md', r"a maker forwarding a governor's row", 'who may ask for a reclaim in ShaperOS mode'),
    ('docs/MANAGER-PUBLISHER-CONTRACT.md', r'''called it the governor before that date|"governor" alone now means the SHAPER canon's governor''',
     'the naming note that maps the former word to the active manager'),
    # A citation of the fencing laboratory's model, whose class for the node is `Maker`.
    ('*', r'''\(the model's "maker"\)''', "the fencing laboratory model's own word for the node, quoted as such"),
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
