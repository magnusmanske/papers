#!/bin/bash
toolforge jobs delete rustbot
# toolforge jobs run --mem 5000Mi --cpu 3 --continuous --mount=all \
# --image tool-sourcemd/tool-sourcemd:latest \
# --command "sh -c 'target/release/main bot --config /data/project/sourcemd/rust/papers/bot.ini'" \
# rustbot
toolforge jobs run --mem 3000Mi --cpu 2 --mount=all --image tool-sourcemd/tool-sourcemd:latest \
--command "sh -c 'target/release/main bot --config /data/project/sourcemd/rust/papers/bot.ini'" \
rustbot
