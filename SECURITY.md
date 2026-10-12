# Security

## Supported versions

Fixes go into the latest release. Please update before you report.

## Reporting a vulnerability

Please do not open a public issue for a security problem.

Report it in private through GitHub: open the
[Security tab](https://github.com/onehorizonai/domainjaeger/security) of this
repository and choose **Report a vulnerability**. Only the maintainers can see
what you send.

Tell us:

- what the problem is and what someone could do with it;
- how to reproduce it, as exactly as you can;
- the version (`domainjaeger --version`) and your system.

You will get an answer as soon as a maintainer has read it. Once a fix is
released, the report is published with credit to you, unless you would rather
stay unnamed.

## What domainjaeger touches

It helps to know what the program does when you judge a finding:

- It sends requests to Wiktionary and WikiWoordenboek (Wikimedia), to IANA and
  ICANN for the list of extensions, to the public DNS resolvers of Google and
  Cloudflare (over HTTPS), and to the RDAP and WHOIS servers of registries.
- It stores its results in a SQLite file in your home folder
  (`~/.domainjaeger/cache.sqlite`), together with the contact you may have
  given. That contact is sent to Wikimedia only, in the User-Agent.
- It writes an Excel file when you ask it to.
- It needs no account and holds no passwords or keys.
