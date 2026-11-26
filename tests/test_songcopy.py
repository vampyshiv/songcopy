"""Tests for songcopy. Run:  python3 -m unittest discover -s tests

Builds a small fake music library (empty files with real-looking names) in a temp
folder, so the tests never touch your real music.
"""
import importlib.util
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("songcopy", os.path.join(HERE, "..", "songcopy.py"))
sc = importlib.util.module_from_spec(spec)
sys.modules["songcopy"] = sc
spec.loader.exec_module(sc)

LIBRARY = [
    "Me/Depeche Mode/Violator (1990)/Depeche Mode - Violator - 06 - Enjoy the Silence.flac",
    "Me/Depeche Mode/Violator (1990)/Depeche Mode - Violator - 06 - Enjoy the Silence.mp3",
    "Me/Depeche Mode/Enjoy the Silence (1990)/Depeche Mode - Enjoy the Silence - 03 - Enjoy the Silence (Hands and Feet mix).flac",
    "Me/Depeche Mode/Touring the Angel - Live (2006)/Depeche Mode - Touring the Angel - Live - 03 - Enjoy the Silence.flac",
    "Me/Emptyset/Avichi EP (2011)/Emptyset - Avichi EP - 01 - Avichi (original mix).flac",
    "Me/Emptyset/Avichi EP (2011)/Emptyset - Avichi EP - 02 - Avichi (Monoloc remix).flac",
    "Me/HEALTH/GET COLOR (2009)/HEALTH - GET COLOR - 09 - IN VIOLET.flac",
    "Me/HEALTH/-DISCO2 (2010)/HEALTH - -DISCO2 - 08 - In Violet (Salem remix).flac",
    "Me/Robyn/Honey (Joe Goddard remix) (2019)/Robyn - Honey (Joe Goddard remix) - 02 - Honey (Joe Goddard remix).flac",
    "Me/Robyn/Robyn Is Here (1995)/Robyn - Robyn Is Here - 08 - Don't Want You Back.flac",
    "Me/Rammstein/Sehnsucht (1997)/Rammstein - Sehnsucht - 05 - Engel (extended version).flac",
    "Me/Sigur Rós/Valtari (2012)/Sigur Rós - Valtari - 04 - Rembihnútur.mp3",
    "Me/Silent Servant - Harm In Hand - Techno Cartel.mp3",
    "Podcasts/Some Show/Episode 1.mp3",
]


class Base(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.root = os.path.join(cls.tmp.name, "Music")
        for rel in LIBRARY:
            p = os.path.join(cls.root, rel)
            os.makedirs(os.path.dirname(p), exist_ok=True)
            with open(p, "wb") as fh:
                fh.write(rel.encode())          # unique content + size per file
        os.environ["LOCALAPPDATA"] = os.path.join(cls.tmp.name, "cache")   # keep the index cache out of ~
        cls.lib = sc.Library(cls.root)
        cls.lib.scan()

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    @staticmethod
    def read(p):
        with open(p, "rb") as fh:
            return fh.read()

    def m(self, artist, title, prefer="flac"):
        return self.lib.match({"artist": artist, "title": title}, prefer)


class Scanning(Base):
    def test_skips_podcasts(self):
        self.assertEqual(len(self.lib.files), len(LIBRARY) - 1)

    def test_cache_round_trip(self):
        lib2 = sc.Library(self.root)
        self.assertTrue(lib2.load_cache())
        self.assertEqual(len(lib2.files), len(self.lib.files))


class Matching(Base):
    def assertMatch(self, r, status, title=None, ext=None):
        self.assertEqual(r["status"], status, r)
        if title is not None:
            self.assertEqual(r["file"]["title"], title)
        if ext is not None:
            self.assertEqual(r["file"]["ext"], ext)

    def test_exact_prefers_studio_original_and_flac(self):
        self.assertMatch(self.m("Depeche Mode", "Enjoy the Silence"), "found", "Enjoy the Silence", "flac")
        self.assertNotIn("Live", self.m("Depeche Mode", "Enjoy the Silence")["file"]["rel"])

    def test_prefer_mp3(self):
        self.assertMatch(self.m("Depeche Mode", "Enjoy the Silence", "mp3"), "found", ext="mp3")

    def test_sloppy_spelling(self):
        self.assertMatch(self.m("sigur ros", "rembihnutur"), "found", "Rembihnútur")
        self.assertMatch(self.m("Robyn", "Don't Want You Back - Remastered 2011"), "found", "Don't Want You Back")

    def test_typo_is_a_best_guess(self):
        self.assertMatch(self.m("Depeche Mode", "Enjoy the Slience"), "guess", "Enjoy the Silence")

    def test_specific_remix(self):
        self.assertMatch(self.m("Depeche Mode", "Enjoy the Silence - Hands and Feet Mix"), "found",
                         "Enjoy the Silence (Hands and Feet mix)")
        self.assertMatch(self.m("HEALTH", "In Violet (Salem Remix)"), "found", "In Violet (Salem remix)")

    def test_original_mix_is_the_plain_song(self):
        self.assertMatch(self.m("Emptyset", "Avichi"), "found", "Avichi (original mix)")
        self.assertMatch(self.m("Emptyset", "Avichi (Original Mix)"), "found", "Avichi (original mix)")

    def test_extended_mix_equals_extended_version(self):
        self.assertMatch(self.m("Rammstein", "Engel - Extended Mix"), "found", "Engel (extended version)")

    def test_version_not_owned(self):
        r = self.m("HEALTH", "In Violet (Extended Mix)")
        self.assertMatch(r, "version", "IN VIOLET")            # offers the plain original first
        self.assertMatch(self.m("Robyn", "Honey"), "version", "Honey (Joe Goddard remix)")

    def test_missing(self):
        r = self.m("Queen", "Bohemian Rhapsody")
        self.assertEqual(r["status"], "missing")
        self.assertIsNone(r["file"])

    def test_no_unrelated_suggestions(self):
        r = self.m("Queen", "Bohemian Rhapsody (Extended Mix)")
        self.assertEqual(r["status"], "missing")
        self.assertEqual(r["suggestions"], [])

    def test_free_text_line_and_filename(self):
        self.assertEqual(self.lib.match({"raw": "Robyn - Don't Want You Back"})["status"], "found")
        r = self.lib.match({"raw": "Silent Servant - Harm In Hand - Techno Cartel.mp3"})
        self.assertEqual(r["status"], "found")


class ReadingLists(unittest.TestCase):
    def test_csv_with_spotify_headers(self):
        rows = sc.read_rows("x.csv", b"Track Name,Artist Name(s),Album Name\nHoney,Robyn,\n")
        self.assertEqual((rows[0]["artist"], rows[0]["title"]), ("Robyn", "Honey"))

    def test_pasted_from_spreadsheet(self):
        rows = sc.read_rows("pasted.txt", b"Robyn\tHoney\n")
        self.assertEqual((rows[0]["artist"], rows[0]["title"]), ("Robyn", "Honey"))

    def test_plain_lines_with_commas(self):
        rows = sc.read_rows("pasted.txt", "Simon & Garfunkel - The Boxer, Part 2\n".encode())
        self.assertEqual(rows[0]["raw"], "Simon & Garfunkel - The Boxer, Part 2")

    def test_m3u(self):
        rows = sc.read_m3u("#EXTM3U\n#EXTINF:1,Robyn - Honey\nC:\\Music\\x.mp3\n")
        self.assertEqual((rows[0]["artist"], rows[0]["title"]), ("Robyn", "Honey"))

    def test_split_version(self):
        self.assertEqual(sc.split_version("Avichi (Original Mix)"), ("Avichi", ""))
        self.assertEqual(sc.split_version("In Violet - Salem Remix"), ("In Violet", "salemremix"))
        self.assertEqual(sc.split_version("(I Just) Died in Your Arms")[0], "(I Just) Died in Your Arms")


class Copying(Base):
    def test_never_overwrites_and_never_touches_source(self):
        src = os.path.join(self.root, LIBRARY[0])
        before = self.read(src)
        with tempfile.TemporaryDirectory() as dest:
            plan = sc.plan_copies([{"src": src, "rel": LIBRARY[0]}], dest)
            self.assertEqual(sc.copy_one(*plan[0])[0], "copied")
            self.assertEqual(sc.copy_one(*plan[0])[0], "skipped")          # same file again
            other = os.path.join(dest, os.path.basename(src))
            with open(other, "wb") as fh:
                fh.write(b"something else")                                # different file, same name
            status, final, _ = sc.copy_one(*plan[0])
            self.assertEqual(status, "copied")
            self.assertTrue(final.endswith(" (2).flac"))
            self.assertEqual(self.read(other), b"something else")  # not overwritten
        self.assertEqual(self.read(src), before)                    # library untouched
        self.assertTrue(os.path.exists(src))

    def test_plan_numbering_and_name_clashes(self):
        items = [{"src": "/a/x/01 - Intro.flac", "rel": "x/01 - Intro.flac"},
                 {"src": "/a/y/01 - Intro.flac", "rel": "y/01 - Intro.flac"},
                 {"src": "/a/x/01 - Intro.flac", "rel": "x/01 - Intro.flac"}]   # duplicate request
        plan = sc.plan_copies(items, "/dest")
        self.assertEqual([os.path.basename(d) for _, d in plan], ["01 - Intro.flac", "01 - Intro (2).flac"])
        plan = sc.plan_copies(items[:2], "/dest", number=True)
        self.assertEqual(os.path.basename(plan[0][1]), "001 - 01 - Intro.flac")

    def test_bad_characters_are_made_safe_for_usb_and_windows(self):
        plan = sc.plan_copies([{"src": "/a/What?: A Song.mp3", "rel": ""}], "/dest")
        self.assertEqual(os.path.basename(plan[0][1]), "What__ A Song.mp3")


if __name__ == "__main__":
    unittest.main()
