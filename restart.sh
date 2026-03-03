#!/bin/bash
toolforge jobs delete rustbot
toolforge jobs run --mem 5000Mi --cpu 3 --continuous --mount=all \
--image tool-sourcemd/tool-sourcemd:latest \
--command "sh -c 'target/release/main bot' /data/project/sourcemd/rust/papers/bot.ini" \
rustbot
