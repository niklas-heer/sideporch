# Vendored scripts

## mermaid-12.0.0.min.js.gz

[Mermaid](https://mermaid.js.org) 12.0.0 (MIT), which draws diagrams in
```` ```mermaid ```` code blocks. `assets/app.js` loads it only on pages that
show a diagram. It is `dist/mermaid.min.js` from the npm package, compressed
with `gzip -9 -n`, and served with `Content-Encoding: gzip`.

- npm integrity: `sha512-/wQXC9iBxoGV8p3erbvaXs9h77VyLDBH6GdayVjj3hEcSQhFU4N1WUhUppotCEqlIxI2pRMwjwBSwTB1MfZBgQ==`
- SHA-256 of `mermaid.min.js`: `28fca7ae6ebc7ed7bb63bde63136a74bfef14f296a57e403657eeb8b32836073`

To update, download the new package, check its integrity against
`npm view mermaid@VERSION dist.integrity`, check that the bundle uses neither
`eval` nor `new Function` (the page's Content Security Policy forbids them),
compress it the same way, and update the file name in `src/assets.rs`.
