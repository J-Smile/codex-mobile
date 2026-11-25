#!/bin/sh
cd "$(dirname "$0")" || exit 1
npm start
codex_mobile_exit=$?
if [ "$codex_mobile_exit" -ne 0 ]; then
  printf '\n%s\n' '启动失败，按回车关闭窗口。'
  read -r codex_mobile_reply
fi
exit "$codex_mobile_exit"
