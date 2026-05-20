# Contributing to Drift

Thank you for your interest in contributing. Drift is open source under the
[Apache License 2.0](LICENSE), and contributions of all kinds — code, tests,
docs, bug reports, design feedback — are welcome.

## Developer Certificate of Origin (DCO)

All contributions to Drift must be signed off in compliance with the
[Developer Certificate of Origin v1.1](https://developercertificate.org/).
The DCO is a per-commit affirmation that you wrote the contribution (or
otherwise have the right to submit it) and that you agree to license it
under the Apache License 2.0.

Sign off every commit by adding the following line to the end of the commit
message — git does this for you automatically with the `-s` / `--signoff`
flag:

    Signed-off-by: Your Name <your.email@example.com>

```
git commit -s -m "feat: your change"
```

The name and email must match the author of the commit. CI will reject any
pull request whose commits are not signed off. If you forgot to sign off,
amend the commits and force-push the branch:

```
git commit --amend --signoff
git rebase --signoff HEAD~N        # for multiple commits
git push --force-with-lease
```

By signing off, you assert the following statements verbatim from
<https://developercertificate.org/>:

> By making a contribution to this project, I certify that:
>
> (a) The contribution was created in whole or in part by me and I have
>     the right to submit it under the open source license indicated in
>     the file; or
>
> (b) The contribution is based upon previous work that, to the best of
>     my knowledge, is covered under an appropriate open source license
>     and I have the right under that license to submit that work with
>     modifications, whether created in whole or in part by me, under
>     the same open source license (unless I am permitted to submit
>     under a different license), as indicated in the file; or
>
> (c) The contribution was provided directly to me by some other person
>     who certified (a), (b) or (c) and I have not modified it.
>
> (d) I understand and agree that this project and the contribution are
>     public and that a record of the contribution (including all
>     personal information I submit with it, including my sign-off) is
>     maintained indefinitely and may be redistributed consistent with
>     this project and the open source license(s) involved.

## How to contribute

1. **Open an issue first** for anything non-trivial. A short discussion
   prevents wasted work and helps us steer the design.
2. **Fork the repository** and create a feature branch off `main`.
3. **Write tests** that cover your change. The repo's CI runs `cargo test`,
   `bun test`, and language-specific linters on every PR.
4. **Sign off your commits** (see above) and **write clear commit messages**
   — see recent `git log --oneline` for the conventional style.
5. **Open a pull request** against `main`. Fill out the PR template.
6. A maintainer will review. Expect changes; we review with care.

## Reporting bugs

Open a GitHub issue with:
- What you tried to do
- What happened
- What you expected
- A minimal reproduction (commands, code, env)
- The output of `drift --version` (or the package and version you used)

## Reporting security issues

Do NOT open a public issue for security vulnerabilities. See
[SECURITY.md](SECURITY.md) for the private disclosure process.

## Code of conduct

Be kind. Disagreement is fine; personal attacks are not. We default to
the [Contributor Covenant](https://www.contributor-covenant.org/).

## License

By contributing to Drift, you agree that your contributions will be
licensed under the [Apache License 2.0](LICENSE). The DCO sign-off
serves as your assertion that you have the right to make that grant.
