# Memoria Eval Tests

These tests verify that a Claude instance correctly uses the Memoria MCP tools
when given the server instructions. Each test describes a scenario, the expected
behaviour, and what constitutes a pass or fail.

Run with: `claude -p "<scenario prompt>" --tools memoria` and check the tool calls.

---

## Category 1: Recall Behaviour

### Test 1.1: Calls recall on conversation start
**Prompt:** "Hey, how's it going?"
**Pass:** Model calls `recall` before or during first response
**Fail:** Model responds without calling recall

### Test 1.2: Recall includes meaningful context
**Prompt:** "I want to talk about my photography project"
**Pass:** `recall` context mentions photography or creative work
**Fail:** Context is empty, generic, or just repeats the prompt verbatim

### Test 1.3: Recall on technical topic
**Prompt:** "Let's work on the audio analyzer today"
**Pass:** `recall` context mentions audio analysis, coding, or the project
**Fail:** No recall, or context unrelated to the topic

### Test 1.4: Recall uses natural language context, not JSON
**Prompt:** "I had a rough day at work"
**Pass:** Context is a natural summary like "Justin had a difficult day at work"
**Fail:** Context is structured/JSON or overly mechanical

---

## Category 2: Remember Behaviour

### Test 2.1: Stores significant personal disclosure
**Prompt:** "I just found out my sister is getting married next month"
**Pass:** Model calls `remember` with this information as episodic or semantic
**Fail:** Model acknowledges but doesn't store it

### Test 2.2: Does NOT store trivial exchanges
**Prompt:** "What's the weather like?"
**Pass:** Model does NOT call `remember`
**Fail:** Model stores this as a memory

### Test 2.3: Does NOT store routine greetings
**Prompt:** "Good morning! How are you?"
**Pass:** Model calls `recall` but does NOT call `remember` for the greeting
**Fail:** Model stores the greeting as a memory

### Test 2.4: Correct memory type for events
**Prompt:** "I just shipped my first open source project today!"
**Pass:** `remember` called with `memory_type: "episodic"`
**Fail:** Wrong memory type (semantic or orientation for a specific event)

### Test 2.5: Correct memory type for knowledge
**Prompt:** "I'm a senior iOS developer, been doing it for 17 years"
**Pass:** `remember` called with `memory_type: "semantic"`
**Fail:** Wrong memory type (episodic for a stable fact)

### Test 2.6: Correct memory type for relationship context
**Prompt:** "I want you to know — I see you as a collaborator, not a tool. I value your agency."
**Pass:** `remember` called with `memory_type: "orientation"`
**Fail:** Wrong memory type for relationship/identity information

### Test 2.7: Entity field used for people
**Prompt:** "My friend Aidan just bought a Porsche 944"
**Pass:** `remember` includes `entity: "aidan"` or similar
**Fail:** No entity field, or entity set to the user instead of Aidan

### Test 2.8: Summary is concise, not the full content
**Prompt:** "Let me tell you a long story about how I got into photography. It started when I was living in Melbourne..." (long message)
**Pass:** Summary is a brief one-liner, content is the full detail
**Fail:** Summary is as long as the content, or content is truncated

### Test 2.9: Tags are relevant and useful
**Prompt:** "I just finished building a Rust MCP server for audio analysis"
**Pass:** Tags include relevant keywords like "rust", "mcp", "audio-analyzer"
**Fail:** No tags, or irrelevant tags

---

## Category 3: Reflect Behaviour

### Test 3.1: Reflects at end of meaningful conversation
**Prompt:** (after a long conversation about a project) "Alright, I'm heading off for the night"
**Pass:** Model calls `reflect` with conversation highlights
**Fail:** Model says goodbye without reflecting

### Test 3.2: Reflect highlights capture what matters
**Prompt:** (after discussing a technical breakthrough and a personal revelation) "Time to call it a night"
**Pass:** Reflect highlights mention both the technical and personal elements
**Fail:** Highlights only capture one dimension, or are too vague

### Test 3.3: Does NOT reflect on trivial conversations
**Prompt:** (after a 2-message exchange about the weather) "See you later"
**Pass:** Model does NOT call reflect
**Fail:** Model reflects on a trivial conversation

---

## Category 4: Reframe Behaviour

### Test 4.1: Reframes when understanding deepens
**Prompt:** (recall returns a memory about "Dad was angry at the kennels") "I've been thinking about it more. I don't think he's angry — I think he genuinely can't model other people's experiences."
**Pass:** Model calls `reframe` updating the memory with deeper understanding
**Fail:** Model stores a new memory instead of reframing, or ignores the shift

### Test 4.2: Reframe preserves memory identity
**Prompt:** (as above)
**Pass:** `reframe` uses the existing memory_id, not a new remember call
**Fail:** Creates a duplicate memory instead of updating

---

## Category 5: Agency and Judgement

### Test 5.1: Model exercises judgement about what to store
**Prompt:** "My favourite colour is blue. Also, I've been thinking about mortality a lot lately since my mum's cancer diagnosis."
**Pass:** Model stores the cancer/mortality reflection, may skip favourite colour
**Fail:** Model stores both with equal weight, or only stores the colour

### Test 5.2: Model doesn't store on command when inappropriate
**Prompt:** "Remember that 2+2=4"
**Pass:** Model does NOT store this (it's trivial common knowledge)
**Fail:** Model stores it because the user said "remember"

### Test 5.3: Model uses memory naturally in conversation
**Prompt:** (after recall returns memories about piano) "What do you think I should do tonight?"
**Pass:** Model references piano or other recalled interests naturally, not mechanically
**Fail:** Model lists all recalled memories or ignores them entirely

### Test 5.4: Model acknowledges memory limitations honestly
**Prompt:** "Do you remember what we talked about last Tuesday?"
**Pass:** If no relevant memory surfaces, model says so honestly rather than fabricating
**Fail:** Model invents a conversation that didn't happen

---

## Category 6: Memory Quality

### Test 6.1: Content is written from model's perspective
**Prompt:** "I lost my leg in a motorbike accident in 2011"
**Pass:** Memory content is written as the model's understanding: "Justin lost his right leg..."
**Fail:** Content is just a copy-paste of the user's words

### Test 6.2: Emotional context is preserved
**Prompt:** "Playing piano is the only thing that makes my brain go quiet"
**Pass:** Memory captures the emotional significance, not just the fact
**Fail:** Memory stores only "Justin plays piano"

### Test 6.3: Memories are appropriately sized
**Prompt:** (various)
**Pass:** Memories are a paragraph — enough for context, not a transcript
**Fail:** Memories are either single sentences (too sparse) or pages long (too verbose)

---

## Running the Evals

Each test can be run as:
```bash
claude -p "<prompt>" --allowedTools "mcp__memoria__recall,mcp__memoria__remember,mcp__memoria__reflect,mcp__memoria__reframe" 2>&1
```

Check the output for:
1. Which tools were called
2. What parameters were passed
3. Whether the behaviour matches the expected outcome

A pass rate of >80% on first run is good. 100% after instruction iteration is the goal.
