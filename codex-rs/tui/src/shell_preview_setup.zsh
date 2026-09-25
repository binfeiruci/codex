SAVEHIST=0

function _codex_preview_snapshot {
  local original_buffer="$BUFFER"
  local original_cursor=$CURSOR
  local suggestion suffix
  local -a current_regions accepted_regions
  if (( ${+functions[_zsh_highlight]} )); then
    _zsh_highlight
  fi
  current_regions=("${region_highlight[@]}")
  POSTDISPLAY=
  if (( ${+functions[_zsh_autosuggest_fetch_suggestion]} )); then
    _zsh_autosuggest_fetch_suggestion "$BUFFER"
    _zsh_autosuggest_suggest "$suggestion"
  fi
  suffix="$POSTDISPLAY"
  if [[ -n "$suffix" ]] && (( ${+functions[_zsh_highlight]} )); then
    BUFFER+="$suffix"
    CURSOR=${#BUFFER}
    POSTDISPLAY=
    _zsh_highlight
    accepted_regions=("${region_highlight[@]}")
  else
    accepted_regions=("${current_regions[@]}")
  fi
  BUFFER="$original_buffer"
  CURSOR=$original_cursor
  POSTDISPLAY="$suffix"
  {
    printf 'P\0%s\0%s\0%s\0%s\0' "$BUFFER" "$CURSOR" "$POSTDISPLAY" "$ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE"
    for region in $current_regions; do
      printf '%s\0' "$region"
    done
    printf 'A\0'
    for region in $accepted_regions; do
      printf '%s\0' "$region"
    done
    printf 'E\0'
  } > "$CODEX_PREVIEW_RESULT_FILE"
}

function _codex_preview_set_line {
  local fd line cursor
  exec {fd}< "$CODEX_PREVIEW_INPUT_FILE"
  IFS= read -r -d '' line <&$fd
  IFS= read -r -d '' cursor <&$fd
  exec {fd}<&-
  BUFFER="$line"
  CURSOR=$cursor
  _codex_preview_snapshot
}

zle -N _codex_preview_set_line
bindkey '^X^F' _codex_preview_set_line
printf R > "$CODEX_PREVIEW_RESULT_FILE"
