# ---- serpl-completions.nu ----
# Directory-only completion helper
def "nu-complete serpl dirs" [] {
  ls -a | where type == "dir" | get name
}

# Extern signatures with completions
export extern "serpl-anywhere" [
  --from: path@"nu-complete serpl dirs"   # start dir for root detection
  --root: path@"nu-complete serpl dirs"   # explicit root dir
]

export extern "serpl-at" [
  path: path@"nu-complete serpl dirs"
]

export extern "serpl-here" []