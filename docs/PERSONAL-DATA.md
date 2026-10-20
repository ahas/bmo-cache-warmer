# Personal data

The repository holds nothing that belongs to a person, a machine or a network. A value that differs
by deployment belongs to that deployment's environment or arguments, and the repository keeps only a
neutral example.

## What the repository may not hold

- A person's name, handle, account id or display name.
- A mail address.
- A telephone number.
- A home directory path.
- A host name, a domain name or a machine name.
- A device, a network address or another detail of a network.
- A company's name.
- A line of a real conversation.

## Where the rule applies

- Code, comments and strings.
- Documents and the README.
- Test fixtures and examples.
- Commit messages, branch names and tags.
- Anything published from the repository.

## What goes in its place

- A value the service needs at runtime comes from its deployment's environment or arguments, never
  from the code.
- An example is neutral: a role such as `tester` or `qa-admin`, a name reserved for documentation
  such as `example.org`, never a real name, account, address or a line someone actually wrote.
- A person, a machine or a network is named by its role, not by the name it carries.

## The values the repository allows

None. The repository names no person, account, host or address of its own.

## The check

The personal-data check in `tests/` reads every file of the checkout at any depth, the build output
and git's directory excepted, and fails on:

- a mail address;
- a home path that names an account, on a Unix-like system or a Windows one;
- a telephone number written in groups, with an optional country or area code;
- a token: a long value assigned to a token, key, secret or password name, or the shape a vendor
  gives its keys;
- a private network address in the RFC 1918 ranges or the link-local range. A loopback address is
  neither and passes.

A failure names the file, the line and the kind, never the value it found. The check keeps no list
of personal terms: it matches shapes. A rule the check cannot read, such as a name written into a
commit message, is held by review.
