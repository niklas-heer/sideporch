# The live demo on Fly.io

<https://sideporch-demo.fly.dev> is a Sideporch server anyone can join. It runs
the release image with [demo mode](https://sideporch.app/docs/community/server/demo-mode/)
on, so it starts over every day at 04:00 UTC.

## Deploy a new release

Change the version in `Dockerfile`, then from this directory:

```sh
fly deploy
```

`fly deploy --build-arg VERSION=main` deploys the newest build of `main` instead, to try something before a release.

## Set it up from scratch

The setup page asks for a setup code, kept in 1Password as
"Sideporch demo setup code". Import it as a Fly secret, so it never
lands in shell history or process arguments:

```sh
fly apps create sideporch-demo
fly volumes create sideporch_data --region fra --size 1 --app sideporch-demo
printf 'SIDEPORCH_SETUP_CODE=%s\n' "$(op read 'op://Private/Sideporch demo setup code/password')" \
  | fly secrets import --stage --app sideporch-demo
fly deploy
fly ips list --app sideporch-demo   # if empty: fly ips allocate-v6, fly ips allocate-v4 --shared
```

Open <https://sideporch-demo.fly.dev>, enter the setup code, and create the
admin account (its password goes into 1Password as "Sideporch demo admin").
Then:

1. **Admin → Community**: sign-up **Anyone can sign up**, with rules.
2. Create `#announcements`, where only managers post, and pin a welcome.
3. **Admin → Demo**: switch on the daily reset and keep `#general` and `#announcements`.
4. Give the people who help run the demo a role, so resets keep them.

Check that setup is done: `curl -sI https://sideporch-demo.fly.dev/` should
redirect to `/login`, not `/setup`. The daily reset keeps admins, so setup
never opens again; the code stays set in case the volume is ever lost.

## A custom domain

To serve it at `demo.sideporch.app`, add a `CNAME` from `demo` to
`sideporch-demo.fly.dev` in the `sideporch.app` zone, run
`fly certs add demo.sideporch.app --app sideporch-demo`, and change
`SIDEPORCH_PUBLIC_URL` above and `demo` in `website/zola.toml`.
