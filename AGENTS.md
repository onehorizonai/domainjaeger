# AGENTS.md

This page is for AI agents that work on domainjaeger's code, or run it for
someone. People start with [README.md](README.md) and
[CONTRIBUTING.md](CONTRIBUTING.md).

domainjaeger is a command-line app written in Rust. It reads words from a
dictionary, makes a domain name of each word and an extension (cook.ing,
sn.app), and asks the registry whether that name is registered. Answers go in
a SQLite cache and export to Excel.

## Rules

- **Test on a scratch cache.** A person's finds are in
  `~/.domainjaeger/cache.sqlite`. Pass `--db <scratch file>` on every run,
  unless they ask you to hunt for them.
- **Keep finds private.** Do not put names from someone's cache in a commit,
  an issue, a pull request or a picture.
- **Go easy on the services.** Anonymous clients get 10 dictionary requests a
  minute from Wikimedia, and registries limit lookups as well. A round or two
  is a fine test. A loop is not, and neither are runs side by side.
- **Stay anonymous.** `--contact` and `DOMAINJAEGER_CONTACT` send an address
  to Wikimedia and store it in the cache. Set one only when it is the person's
  own and they ask for it. In examples, write `you@example.com`.
- **Say "shows as available".** domainjaeger knows that a name is not
  registered. It does not know that the name can be bought. See
  [What "available" means](#what-available-means).
- **Leave releases to a person.** A tag that starts with `v` publishes a
  release. `.github/workflows/release.yml` is generated: change
  `dist-workspace.toml` and run `dist generate`. See
  [docs/RELEASING.md](docs/RELEASING.md).

## Build and check

```sh
cargo run --release -- --db /tmp/scratch.sqlite    # the app, on a scratch cache
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                                         # offline, a few seconds
```

The three checks must pass before a pull request. CI runs them on macOS and
Linux. It builds and tests on Windows too, where a failure is only a warning.

A person has run domainjaeger on macOS (Apple Silicon) only. Say which system
you tested on.

## Run it without the home screen

The home screen needs a terminal and arrow keys. Use subcommands and flags
instead.

```sh
DB=/tmp/scratch.sqlite
domainjaeger --db "$DB" check snap today                      # try words of your own
domainjaeger --db "$DB" hunt --rounds 1                       # one round, then exit
domainjaeger --db "$DB" hunt --rounds 2 --export names.xlsx   # two rounds, then Excel
domainjaeger --db "$DB" export names.xlsx                     # Excel from the cache
domainjaeger --db "$DB" refresh                               # verify expired names again
domainjaeger --db "$DB" stats
printf 'c snap\nl\nq\n' | domainjaeger --db "$DB"             # typed commands, line by line
```

Every setting is a flag, and flags go with any subcommand. `domainjaeger
--help` lists them. Typing `h` lists the commands a person types.

When the input is not a terminal, or the window is smaller than 80 columns by
24 lines, domainjaeger shows the setup once and reads commands line by line.

To test the home screen itself, run the program in a pseudo-terminal and read
the screen from a terminal emulator. [CONTRIBUTING.md](CONTRIBUTING.md) tells
how, under Testing the menu. `assets/screenshots.py` is a working example.

## Read the output

| sign | means |
| --- | --- |
| ■ in green | available: the registry has no such registration |
| □ | taken |
| ▒ | unclear: tried again later |
| ⚠ | the word can also mean something rude |
| `$` `>` `#` `@` | the category: finance, tech, product, names |

A result has four columns: the name, the word it is made of, what the word is
about, and the spelling. The spelling is the exact word, or the small typo
that makes it fit. Under an available name is what the word means.

### What "available" means

- **Not registered at the registry.** A registrar may still refuse the name
  (reserved or premium names, residency rules of a country extension) or
  charge a premium. Tell the person to check with a registrar.
- **English names are picked over.** In test runs roughly 6 of every 7
  well-known English words were taken, and short ones that do show up as
  available (spr.ing, jo.in) may well be reserved or premium-priced by the
  registry. Treat them as leads.
- **One answer is known to be false.** The `.in` registry answers "not
  registered" for two-letter names it holds back, like jo.in and sk.in. Its
  WHOIS calls them restricted. domainjaeger shows them as available.
- **A ⚠ means "look twice", not "this word is rude".** A word is flagged when
  one of its senses is: `cock` is a rooster too. And no flag is no guarantee:
  volunteers label what they get to. Dutch has no labels for offensive and
  violent words, and the flags say nothing about what a name sounds like in
  another language.

## Change the code

Read [CONTRIBUTING.md](CONTRIBUTING.md) first. It has the map of the code,
file by file, the style of the screen, and what a pull request should look
like. In short:

- **The outside world sits behind traits** in `src/net.rs`: `Web`, `Whois` and
  `Clock`. Tests hand in stand-ins that answer from a script, so they need no
  network and no waiting. Give anything new that talks to the outside the same
  treatment.
- **Every setting works three ways:** a line in the setup with a list of
  choices, a command you can type, and a flag.
- **The screen is calm and plain.** "Extension", not TLD. "Parts", not
  syllables. Nothing starts on its own. No icons or emoji in the menu. Only
  characters every font has: letters, punctuation, box drawing and block
  elements.
- **The rules are small tables in the code.** The topics behind each category
  are in `src/topics.rs`, the labels that make a word rude in `src/tone.rs`,
  the typos in `src/hacks.rs`, and the extensions each language favours in
  `src/tld.rs`. The Excel columns are in `src/export.rs`.
- **One subject per pull request.** Add or update tests for the behaviour you
  change. Say how you tested, and on which system.

### The documentation

Write short sentences in plain words, to "you". Say what the app does before
how it does it. No exclamation marks, no emoji, no hype, and nothing the app
does not do.

When what a person sees or types changes, update with it:

- `README.md`, which stays short. It is for people who install and use the
  app.
- This page, for the detail.
- The map of the code in `CONTRIBUTING.md`.
- `CHANGELOG.md`, with a line under Unreleased.
- The pictures. `python3 assets/screenshots.py` makes the screenshots again.
  See The pictures in the README in [CONTRIBUTING.md](CONTRIBUTING.md).

## How it works

### Words

Words come live from a dictionary. There is no word list in this repository.
Each language keeps its own place in its dictionary.

- **English** (`en`, the default) comes from the English
  [Wiktionary](https://en.wiktionary.org). Its search can match the end of a
  title ("every word ending in -ing"), and it returns the best known words
  first. It has syllable counts for about 79,000 English words.
- **Dutch** (`nl`) comes from [WikiWoordenboek](https://nl.wiktionary.org). It
  keeps a *retrograde* index: all Dutch words sorted by their reversed
  spelling. That makes "every word ending in -ai" one request. The same
  request returns the syllable count, how many Dutch speakers know the word,
  and whether it is in the official Woordenlijst Nederlandse Taal.
- **Latin** (`la`) and **Spanish** (`es`) come from the English Wiktionary as
  well. It has syllable counts for about 34,000 Latin and 53,000 Spanish words
  of up to 3 syllables. Only the form a dictionary lists a word under is used
  (amo, not amas and amat; libro, not libros), and only words that are plain
  a to z: canción and año cannot be part of a domain name. That leaves about
  13,000 Latin and 24,000 Spanish words.

Unless the word length says otherwise, only words of 1, 2 and 3 syllables are
kept.

Meanings come from the English Wiktionary for all four languages, so they are
in English. A meaning is the first sense of the word, cut down to a few words.
It takes one dictionary request per word, which is why taken names do not get
one. An available name that has no meaning yet gets one when you export.

### Categories

Both dictionaries file their words under topics. A category is a few of those
topics under one name: `finance`, `tech`, `product`. Any topic is a category
of its own: `cat music`, or `cat muziek` in Dutch. Capitals and spaces do not
matter, and neither does a last s: `cat animal` is Animals. `cat list` shows
every topic a language has.

A hunt in a category turns the search around. It fetches the words of the
topics and tries every extension on each word. Real spellings and near-perfect
typos (sn.app) go first, best known words first. Sound-alikes and swapped
letters come last. In a hunt without a category, every word is labelled with
finance, tech and product as well, so the Excel file can be filtered on them.

First names are the category `names`: the given names the English Wiktionary
lists per language, short forms like Mike included. There are about 9,800
English ones, 1,100 Spanish, 700 Latin and 560 Dutch. The dictionary counted
the syllables of few names. A name without a count is always in. One that is
known to be too long is left out.

### Names

**Dot inside the word.** The extension finishes the word: `cook.ing`. This is
the default. Typos are one small step away from the real spelling:

| kind | English | Dutch |
| --- | --- | --- |
| doubled letter | snap → sn.app | knap → kn.app |
| dropped double | especially → especia.ly | tobbe → to.be |
| sound-alike (c/k, i/y, s/z) | today → tod.ai | display → displ.ai |
| swapped letters | delicious → delicio.su | strooi → stro.io |

**Whole word before the dot** (`w`, or `--whole`) is for extensions no word
ends in, like `.nl` and `.com`. The name is the word itself (`lemon.nl`) or a
small typo of it. It is hunted in `.nl` unless you name other extensions.
domainjaeger switches to this style by itself when the extensions you picked
have no words left with the dot inside.

The hunt starts with the best known words and works its way down. Per word it
tries the word and its most natural typo first, and the clumsier typos only
after every word had its turn. The typo rules were written with Dutch spelling
in mind:

| kind | example | |
| --- | --- | --- |
| dropped double | appel → apel.nl | natural |
| sound-alike (ij → y, c → k, s/z) | konijn → konyn.nl, circus → cirkus.nl, fiets → fietz.nl | natural |
| doubled consonant between vowels | lening → lenning.nl | natural |
| doubled last letter | knap → knapp.nl | clumsy |
| last letters swapped | lening → lenign.nl | clumsy |

**A word of your own** (`--prefix`, `--suffix`) goes before or after every
word: headspeed, speedbox. Both can be on at once. These are whole names, so
setting one switches the name style. There are no typos of such a name.

### Extensions

Extensions come from IANA (all delegated TLDs), minus brand TLDs (ICANN's
list) and TLDs that turn out to be closed. By default only extensions of 2 to
4 letters are used.

A handful get a turn in every cycle: the ones many words end in and whose
registry answers lookups. Each language has its own, and the home screen
shows them. The others take their turn in between.

- `.io`, `.co` and `.be` are nearly sold out for Dutch words, so they are not
  in the handful. `t io co be` hunts in them anyway.
- 2,700 Latin words end in -us and 4,000 Spanish verbs in -ar. But `.us` is
  for people with a tie to the United States and `.ar` takes an Argentine tax
  number, so they are not in the handful either.

### The check

A name is reported as available only when its registry says so.

1. **DNS.** A domain with name servers is registered. This settles most taken
   names without bothering a registry. The question goes to the public
   resolvers of Google and Cloudflare, over HTTPS.
2. **RDAP at the registry.** "404, no such registration" means available.
3. **WHOIS at the registry**, for extensions without RDAP (`.be`, `.de`,
   `.co`, ...).

Before a registry lookup is trusted for an extension, it is calibrated: it
must call a known registered domain taken and a gibberish one available.
Extensions that fail are skipped.

Registries limit how often you may ask. Google's `.app` and `.dev` and SIDN's
`.nl` allow about one lookup per second. The hunt paces itself and moves on to
other extensions while one is busy. An unclear answer is shown as ▒ and tried
again later.

### Speed

Wikimedia allows 200 requests a minute to clients with contact details in the
User-Agent, and 10 to anonymous ones. Without a contact, domainjaeger
identifies itself by its name, its version and the address of this repository.

### The cache

`~/.domainjaeger/cache.sqlite` remembers every verdict for 4 weeks (28 days).
`--db` or `DOMAINJAEGER_DB` keeps it somewhere else.

Checked names are not looked up again within that time. The position in the
dictionary is saved, so a new session continues where the last one stopped.
The words of a category are kept for 4 weeks as well, so a topic is downloaded
once. `reset` starts the dictionary from the first page again. The finds stay.

## Limits

- **A category is a selection, not a complete list.** Volunteers label the
  words: `bank` has nine topics, `hypotheek` only "Juridisch". Most words have
  no topic at all, so in a hunt without a category most rows have none.
- **Latin and Spanish categories are thin.** Latin has 11 words under Finance
  and 8 under Computing, so `cat finance` or `cat tech` runs dry quickly
  there. Spanish does better (Computing 333, Clothing 244, Finance 101).
  Latin is strong in law, anatomy, plants and mythology.
- **Latin and Spanish lean on English.** The best known pages come first, and
  those are often words English shares (virus, radio, popular). Typo rules
  were written for Dutch and English spelling.
- **How well known a word is** is only known for Dutch, and shown for words
  that at least 76% of the Dutch know. The dictionary request has room for 50
  questions per word, and the lower percentages made way for the topics and
  the rude-word labels.
- **"Best known" is the dictionary's idea of it.** Whole words and names with
  a word of your own in English, Latin and Spanish start with the pages the
  dictionary links to most, minus its own jargon (genitive, plural) and words
  of one or two letters. Everyday words come early (animal, plant, light,
  fire), with some odd ones in between (hanja, esse). A category gives a
  tidier list.
- **First names come in no order of popularity.** Nobody counts how many
  people carry a name in a dictionary. The names whose page is about little
  else come first (Emma, Hans, Julie), which is a fair stand-in. Other
  sources would do better for one country each (the US social security list,
  the Dutch Voornamenbank, the Spanish INE), but none covers four languages,
  Latin least of all, and none can be asked live.
- **Meanings cost a dictionary request per available name.** Unnoticeable
  once the dictionary knows you. Anonymous, with 10 requests a minute, a
  round of 10 names takes about a minute longer.
- **Some registries cannot be asked.** `.es`, `.as`, `.al` and `.na` do not
  answer public lookups, and `.do` gives unclear answers. Names there are
  never reported as available, so Spanish whole words are hunted in `.nl`
  like the rest unless you name other extensions.
- **Whole words in `.nl` are nearly all taken**, and so are many of their
  typos (`sien.nl`, `halo.nl`, `lenning.nl`). In test runs about 1 name in 7
  was free, almost all of them typos. Every free name needs an answer from
  SIDN, which allows about one lookup per 1.5 seconds, so a round takes 15 to
  30 seconds.
