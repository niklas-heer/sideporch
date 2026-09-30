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

```sh
fly apps create sideporch-demo
fly volumes create sideporch_data --region fra --size 1 --app sideporch-demo
fly deploy
fly ips list --app sideporch-demo   # if empty: fly ips allocate-v6, fly ips allocate-v4 --shared
fly machine exec "$(fly machines list --app sideporch-demo --quiet)" "/sideporch setup-link --data /data" --app sideporch-demo
```

Open the setup link to create the admin account, then:

1. **Admin → Community**: sign-up **Anyone can sign up**, with rules.
2. Create `#announcements`, where only managers post, and pin a welcome.
3. **Admin → Demo**: switch on the daily reset and keep `#general` and `#announcements`.
4. Give the people who help run the demo a role, so resets keep them.

## A custom domain

To serve it at `demo.sideporch.app`, add a `CNAME` from `demo` to
`sideporch-demo.fly.dev` in the `sideporch.app` zone, run
`fly certs add demo.sideporch.app --app sideporch-demo`, and change
`SIDEPORCH_PUBLIC_URL` above and `demo` in `website/zola.toml`.
