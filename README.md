# vcflow

Desktop app that walks you through our Git workflow (feature / bug / release / hotfix branches, merge requests) on GitLab and GitHub. macOS and Windows.

## Install

Needs Python 3. macOS has it; on Windows install it from [python.org](https://www.python.org/downloads/).

macOS:

```sh
curl -fsSLO https://raw.githubusercontent.com/kelvinforge/vcflow/main/scripts/install_vcflow.py
python3 install_vcflow.py
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/kelvinforge/vcflow/main/scripts/install_vcflow.py -OutFile install_vcflow.py
python install_vcflow.py
```

The script downloads the latest [Release](https://github.com/kelvinforge/vcflow/releases/latest), checks its SHA-256 and installs it. Read it first if you like: it is one short file using only the Python standard library.

vcflow is not signed with a paid certificate, so the OS warns about it:

- **macOS**: the script clears the quarantine flag for you. If you installed the `.dmg` by hand and see "vcflow can't be opened", run:
  ```sh
  xattr -dr com.apple.quarantine /Applications/vcflow.app
  ```
- **Windows**: if SmartScreen appears, click **More info → Run anyway**.

## Update

From 0.4.8 on, vcflow checks for a new version on startup. When one is out, the footer shows **Update**: click it and the app restarts on the new version.

On 0.4.7 or older, run the install script again.

## First use

1. Open your repository folder.
2. Click the token button in the header and paste a personal access token:
   - **GitLab**: scope `api` (creating merge requests needs it; `read_api` is not enough).
   - **GitHub**: classic token with `repo`, or a fine-grained token for the repo with **Pull requests: Read and write** and **Contents: Read**.
3. The token is stored encrypted on this machine only. On a new machine, enter it again.

## Troubleshooting

- **Token is gone after updating** (macOS, coming from 0.4.6 or older): enter it again once. Tokens moved out of the keychain in 0.4.7.
- **"credential file cannot be decrypted on this machine"**: save the token again. The unreadable file is kept as `credentials.enc.bak`.
- **"can't be opened" / "is damaged"** on macOS: run the `xattr` command above.

## Development

```sh
npm ci
npm run tauri dev
```

Needs Node 20 and Rust stable. Releases are built by `.github/workflows/release.yml` when a `vX.Y.Z` tag is pushed.
