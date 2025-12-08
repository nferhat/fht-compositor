# Contributing on `fht-compositor`
First of all, I'd like to thank you for trying to help out with the compositor development!

Making a Wayland compositor is quite a daunting task, any help is greatly appreciated.

## Pull requests
...are always appreciated! However, keep in mind that I still want a *somewhat* refined final product,
so I <text style="font-size: 10px">now</text> try to maintain quality and consistency with what gets
introduced into the compositor.


### Creating pull requests

When opening PRs, you should always remember that:

- I want at the core that the compositor only handles two things: *compositing* (drawing windows) and
  window management (inside `src/space`), and wish to keep the code complexity low, so don't be surprised
  if I straight out refuse an idea.
  - Ideas that are  "out of scope" should be filtered out by opening a discussion/issue, or by discussing
    it on the matrix/discord server.
- Try to architect your new code in a way that is "in line" with the rest of the compositor codebase. When
  unclear (which is often especially if you are a new contributor), you can always request a review and
  I'll try to give relevant examples.
- Pull requests should always be focused on a single thing, let that be a new feature or bugfix.
- Try to organize your commits into self-contained units
  - Consider squashing commits (to avoid small commits like "Fix constant X here", "Oversight there", ...)
  - `git rebase main` is always preferred to `git merge main`, since it avoids a lot of noise in the history
- Remember to document your new feature/configuration keys/behaviour, and if you create a new wiki page,
  *do not forget* to include it in the sidebar by editing `docs/.vitepress/config.mts`!
- Sometimes I do have some nitpicks regarding code consistency and style, to make it "match more with the
  existing codebase", so make sure to tick "Allow editing by maintainers"

### The review process

Reviewing pull requests, and testing them thoroughly, and especially iterating/tweaking it to arrive to the
final product is a time-consuming task. Following the points above will tremendously help the process.

Additional notes:
- Due to my busy student life, I am not always able to thoroughly test your new feature immediately, it is
  what it is, however I am always trying to get quickly to them (especially with the low volume)
    - If you are someone who has already contributed to the compositor and wish to review, make sure to leave
      a comment in the PR when you are done to let me know!
- For new features, make sure you check edge cases thoroughly
    - Anything related to animations requires a lot of attention, as everything should be smooth (IE no sudden
      jumps in the animation progress). This takes quite a lot of time to iron out. Testing with longer animation
      duration is a good way to figure these kinds of issues.
    - When applicable, unit tests are appreciated (however note that the compositor currently lacks a testing
      framework/method, so this is yet to be worked on...)
- Custom protocols don't need to be generic, just implement a single `refresh` (and others if applicable) functions
  in the `protocols::<name>` module that is specific to `state::State`

For larger feature merges, they do to an additional review phase that consists of me daily driving them on my
devices (both laptop and desktop). This can take as short as one day to as long as one week or more. This is mostly
to make sure that they don't interfere and integrate smoothly into the expected workflow of the compositor.

Even if you are a regular user/non-dev, testing out new features/bugfixes is still super helpful! I do have super
linux-friendly hardware (AMD gpus only, thinkpad laptop...) with a single unique environment (configured through
NixOS with the compositor flake) so testers with more or less exotic setups can help us catch a lot edge cases!
