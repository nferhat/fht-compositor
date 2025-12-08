# Reporting issues
The compositor project being fairly new, and lacking really active users, I can't iron out all bugs with
me as the sole active user. When encountering a bug, creating a [GitHub issue](https://github.com/nferhat/fht-compositor/issues/new) is the first step to get that issue fixed

## Must includes in your issue report
1. Compositor information: version, commit, custom build features if any
```sh
WAYLAND_DEBUG=1 SOME_ENV_VAR_TO_ENABLE_LOGS=1 application > app-logs
```
2. Environment information: distribution, kernel version, how did you install it (package/from source)
3. Essential logs
    * When including them in the issue report, make you to wrap them inside a `<details>` block, and by also
    wrapping them inside a code block.
    * The backtrace of the compositor crash, that you can get with:
        - **systemd installs**:`journalctl --user -xeu fht-compositor.service`
        - **non-systemd**: You should try to reproduce the bug with `fht-compositor-session | tee fhtc.log`
    * If it's an application issue/crash, try to reproduce the bug by running the application through
       your terminal with the following, and attaching the logs:

## Good practices/Tips
- Check duplicates and point them out.
- Always mention the specific steps required to reproduce the issue.
- Check for issues that are not specific to `fht-compositor`, either by checking them on other compositor.
    - It's even better if you can check both *smithay-based* and *non smithay-based* compositors, as it will help us know
      whether the issue is specific to `fht-compositor`, Smithay, or the application itself.
- If you are using `xwayland-satellite`, and the application has both a Wayland and X11 version, make sure
  to run it with `env -u DISPLAY <command>` to make sure that you are testing the Wayland version. Otherwise,
  mention that you are on `xwayland-satellite`.
- Avoid noise in the issue discussions, upvoting/reacting with a thumbs up on the original message is enough to signal
  that the issue affects you, no need to send a message about "Is this still active/known about".
- Sometimes, rendering issues disappear when screen recording is enabled, your only resort in this case is to record the
  screen with your phone or use a capture card (if you have that luxury)

If you are someone checking out another issue, it's always helpful to try and reproduce the issue and reporting that
(by message or by reacting to an existing "Can reproduce" message), sometimes it may be hardware/environment specific and
figuring that out pushes forward the investigation process.
