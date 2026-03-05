#!/bin/bash
toolforge jobs delete rustbot-continuous
toolforge jobs run --mem 2000Mi --cpu 2 --mount=all --continuous --image tool-sourcemd/tool-sourcemd:latest --command "sh -c 'target/release/main bot --config /data/project/sourcemd/rust/papers/bot.ini'" rustbot-continuous
