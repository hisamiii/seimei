## GPU queue
GPU and NPU jobs take turns: one queued command holds the turn, the others wait in order until it ends. mcp__queue__queue shows who holds it and who waits; [seimei] lines tell you when a wait passes 10 minutes. queue_stop stops the holder: use it only when the user asks or a holder is clearly stuck.
