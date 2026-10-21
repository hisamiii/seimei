"""The check scripts moved from tmp/ (2026-10-21), one test each."""
import pytest

from conftest import CLAUDE_DIR, run_script, scripts


FAKE = "/home/tester"  # the home the guard scripts were written against, replaced by a throwaway one


def fake_home(home):
    """A small home with the kinds of files the guard protects, so its globs have something to match."""
    for f in (".ssh/id_ed25519", ".gnupg/pubring.kbx", ".local/share/TelegramDesktop/tdata/key_datas", ".bashrc",
              ".local/share/seimei/auth.json"):
        (home / f).parent.mkdir(parents=True, exist_ok=True)
        (home / f).write_text("test\n")
    (home / "keys").symlink_to(".ssh")
    # More names than a glob looks at on disk: a ** glob must still see the secrets past them.
    (home / "big").mkdir()
    for i in range(6000):
        (home / "big" / str(i)).touch()


@pytest.mark.parametrize("path", scripts("unit"), ids=lambda p: p.stem)
def test_unit(path, tmp_path):
    # A throwaway home: nothing a unit script does may touch the real one.
    home = tmp_path / "home"
    fake_home(home)
    text = path.read_text()
    if FAKE in text:
        path = tmp_path / path.name
        path.write_text(text.replace(FAKE, str(home)))
    run_script(path, HOME=str(home))


@pytest.mark.web
@pytest.mark.parametrize("path", scripts("web"), ids=lambda p: p.stem)
def test_web(path, qa_server):
    run_script(path)


@pytest.mark.live
@pytest.mark.parametrize("path", scripts("live"), ids=lambda p: p.stem)
def test_live(path):
    if not (CLAUDE_DIR / ".credentials.json").exists():
        pytest.skip(f"log the test config in once: CLAUDE_CONFIG_DIR={CLAUDE_DIR} claude /login")
    run_script(path, timeout=900)
