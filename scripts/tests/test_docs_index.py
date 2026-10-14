"""The docs index tables are generated; nothing here touches the network or real配置."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'docs_index.py'
REPO = Path(__file__).resolve().parents[2]

sys.path.insert(0, str(SCRIPT.parent))
# 索引漂移的修复提示。文档本身写得不对时不该出现它：重新生成一遍不会好，要作者改文件。
from docs_index import FIX as REGENERATE  # noqa: E402

DECISIONS_README = """# 架构决定记录

前言。

## 记录

<!-- docs-index:begin -->
{table}
<!-- docs-index:end -->
"""

SUBSYSTEMS_README = """# 子系统设计参考

前言。

<!-- docs-index:begin -->
{table}
<!-- docs-index:end -->

## 维护参考文档

后面还有正文。
"""

DECISION = """# {number}. {title}

- 状态：{status}
- 日期：2026-10-14

## 背景
略。
"""

SUBSYSTEM = """# {title}

> {summary}

正文。
"""


def run(repo, *args):
    return subprocess.run([sys.executable, str(SCRIPT), '--repo', str(repo), *args],
                          capture_output=True, text=True)


class Fixture:
    """A miniature repo: two record files, two index READMEs."""

    def __init__(self, root):
        self.root = Path(root)
        self.decisions = self.root / 'docs' / 'decisions'
        self.subsystems = self.root / 'docs' / 'subsystems'
        self.decisions.mkdir(parents=True)
        self.subsystems.mkdir(parents=True)
        self.add_decision('0001', 'damai-event-data-source', '演出信息从大麦详情页获取', '采用')
        self.add_decision('0002', 'watch-timing', '盯票的定时和轮询', '被 0007 取代')
        self.add_subsystem('damai-event', '大麦演出信息查询', '从大麦读场次、票档和售卖状态')
        self.write_indexes(
            '| 编号 | 标题 | 状态 |\n| --- | --- | --- |\n'
            '| [0001](0001-damai-event-data-source.md) | 演出信息从大麦详情页获取 | 采用 |\n'
            '| [0002](0002-watch-timing.md) | 盯票的定时和轮询 | 被 0007 取代 |',
            '| 子系统 | 参考 |\n| --- | --- |\n'
            '| 从大麦读场次、票档和售卖状态 | [大麦演出信息查询](damai-event.md) |',
        )

    def add_decision(self, number, slug, title, status='采用'):
        (self.decisions / f'{number}-{slug}.md').write_text(
            DECISION.format(number=number, title=title, status=status),
            encoding='utf-8')

    def add_subsystem(self, slug, title, summary):
        (self.subsystems / f'{slug}.md').write_text(
            SUBSYSTEM.format(title=title, summary=summary), encoding='utf-8')

    def write_indexes(self, decisions_table, subsystems_table):
        (self.decisions / 'README.md').write_text(
            DECISIONS_README.format(table=decisions_table), encoding='utf-8')
        (self.subsystems / 'README.md').write_text(
            SUBSYSTEMS_README.format(table=subsystems_table), encoding='utf-8')

    def decisions_index(self):
        return (self.decisions / 'README.md').read_text(encoding='utf-8')

    def subsystems_index(self):
        return (self.subsystems / 'README.md').read_text(encoding='utf-8')

    def index_bytes(self):
        return ((self.decisions / 'README.md').read_bytes(),
                (self.subsystems / 'README.md').read_bytes())


class Check(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.fixture = Fixture(self.tmp.name)

    def test_index_that_matches_the_directory_passes(self):
        result = run(self.fixture.root, '--check')
        self.assertEqual(result.returncode, 0,
                         f'索引和目录一致时不该报错：{result.stdout}{result.stderr}')

    def test_new_record_without_touching_the_index_fails(self):
        self.fixture.add_decision('0003', 'tests-cannot-reach-damai', '测试连不上真实大麦')
        result = run(self.fixture.root, '--check')
        self.assertEqual(result.returncode, 1, '新记录没进索引必须让检查失败')
        self.assertIn('docs/decisions/README.md', result.stdout + result.stderr)
        self.assertIn('0003-tests-cannot-reach-damai.md',
                      result.stdout + result.stderr)
        self.assertIn('docs_index', result.stdout + result.stderr,
                      '失败信息要说明怎么修好')

    def test_new_subsystem_without_touching_the_index_fails(self):
        self.fixture.add_subsystem('release', '发布', '推 tag 发布三个平台的程序')
        result = run(self.fixture.root, '--check')
        self.assertEqual(result.returncode, 1, '新子系统文档没进索引必须让检查失败')
        self.assertIn('docs/subsystems/README.md', result.stdout + result.stderr)
        self.assertIn('release.md', result.stdout + result.stderr)

    def test_row_for_a_deleted_file_fails(self):
        (self.fixture.decisions / '0002-watch-timing.md').unlink()
        result = run(self.fixture.root, '--check')
        self.assertEqual(result.returncode, 1, '索引里留着已删除的记录必须失败')
        self.assertIn('0002-watch-timing.md', result.stdout + result.stderr)

    def test_stale_title_in_the_index_fails(self):
        self.fixture.add_decision('0001', 'damai-event-data-source', '改过的标题')
        result = run(self.fixture.root, '--check')
        self.assertEqual(result.returncode, 1, '标题改了而索引没改必须失败')
        self.assertIn('改过的标题', result.stdout + result.stderr)

    def test_rows_out_of_order_fail(self):
        # 这是 union 合并留下的样子：两行都在，顺序反了。
        self.fixture.write_indexes(
            '| 编号 | 标题 | 状态 |\n| --- | --- | --- |\n'
            '| [0002](0002-watch-timing.md) | 盯票的定时和轮询 | 被 0007 取代 |\n'
            '| [0001](0001-damai-event-data-source.md) | 演出信息从大麦详情页获取 | 采用 |',
            '| 子系统 | 参考 |\n| --- | --- |\n'
            '| 从大麦读场次、票档和售卖状态 | [大麦演出信息查询](damai-event.md) |',
        )
        result = run(self.fixture.root, '--check')
        self.assertEqual(result.returncode, 1, 'union 合并出来的乱序索引必须被拦住')
        self.assertIn('顺序', result.stdout + result.stderr)

    def test_check_does_not_write(self):
        self.fixture.add_decision('0003', 'a-new-one', '新记录')
        before = self.fixture.decisions_index()
        run(self.fixture.root, '--check')
        self.assertEqual(self.fixture.decisions_index(), before,
                         '--check 只比对，不能改文件')


class Write(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.fixture = Fixture(self.tmp.name)

    def test_write_adds_the_new_record_and_check_passes(self):
        self.fixture.add_decision('0003', 'a-new-one', '新记录')
        self.fixture.add_subsystem('release', '发布', '推 tag 发布三个平台的程序')
        written = run(self.fixture.root, '--write')
        self.assertEqual(written.returncode, 0, f'生成失败：{written.stderr}')
        self.assertIn('| [0003](0003-a-new-one.md) | 新记录 | 采用 |',
                      self.fixture.decisions_index())
        self.assertIn('| 推 tag 发布三个平台的程序 | [发布](release.md) |',
                      self.fixture.subsystems_index())
        self.assertEqual(run(self.fixture.root, '--check').returncode, 0,
                         '生成之后检查必须通过')

    def test_write_keeps_the_prose_around_the_table(self):
        self.fixture.add_decision('0003', 'a-new-one', '新记录')
        run(self.fixture.root, '--write')
        self.assertIn('## 维护参考文档', self.fixture.subsystems_index())
        self.assertIn('前言。', self.fixture.decisions_index())
        self.assertTrue(self.fixture.decisions_index().endswith('\n'))

    def test_write_is_idempotent(self):
        self.fixture.add_decision('0003', 'a-new-one', '新记录')
        run(self.fixture.root, '--write')
        once = self.fixture.decisions_index()
        run(self.fixture.root, '--write')
        self.assertEqual(self.fixture.decisions_index(), once,
                         '再生成一次结果必须一样')

    def test_union_merge_leftovers_are_repaired(self):
        self.fixture.add_decision('0003', 'a-new-one', '新记录')
        self.fixture.write_indexes(
            '| 编号 | 标题 | 状态 |\n| --- | --- | --- |\n'
            '| [0003](0003-a-new-one.md) | 新记录 | 采用 |\n'
            '| [0001](0001-damai-event-data-source.md) | 演出信息从大麦详情页获取 | 采用 |\n'
            '| [0002](0002-watch-timing.md) | 盯票的定时和轮询 | 被 0007 取代 |',
            '| 子系统 | 参考 |\n| --- | --- |\n'
            '| 从大麦读场次、票档和售卖状态 | [大麦演出信息查询](damai-event.md) |',
        )
        self.assertEqual(run(self.fixture.root, '--write').returncode, 0)
        self.assertEqual(run(self.fixture.root, '--check').returncode, 0)
        rows = [line for line in self.fixture.decisions_index().splitlines()
                if line.startswith('| [')]
        self.assertEqual(len(rows), 3, 'union 合并留下的重复行要收敛成每个文件一行')
        self.assertTrue(rows[0].startswith('| [0001]'), f'要按编号排序：{rows}')


class Malformed(unittest.TestCase):
    """文档本身写得不对，和索引漂移是两回事：漂移跑一下生成就好，这里要作者改文件。

    每项都用 assert_rejected 断言，因为「退出 1 加文件名」区分不了这两件事：写得不对的
    文档同时也是一个没进索引的新文件，删掉生成器里的格式拒绝之后，它照样让 `--check`
    报漂移、照样退出 1，于是守护被删也没有测试变红（#53）。
    """

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.fixture = Fixture(self.tmp.name)

    def assert_rejected(self, *diagnosis):
        """两个模式都拒绝，说清是哪里写得不对，两个 README 一个字节都不动。"""
        before = self.fixture.index_bytes()
        for mode in ('--check', '--write'):
            result = run(self.fixture.root, mode)
            output = result.stdout + result.stderr
            self.assertEqual(result.returncode, 1, f'{mode} 要拒绝写得不对的文档：{output}')
            for text in diagnosis:
                self.assertIn(text, output, f'{mode} 要说出是哪里写得不对：{output}')
            self.assertNotIn(REGENERATE, output,
                             f'{mode} 报的是索引漂移，不是文档格式：{output}')
        self.assertEqual(self.fixture.index_bytes(), before,
                         '错误输入不能靠先生成糊过去：两张表都要原样不动')

    def test_decision_without_a_title_is_reported(self):
        (self.fixture.decisions / '0003-no-title.md').write_text(
            '- 状态：采用\n\n## 背景\n略。\n', encoding='utf-8')
        self.assert_rejected('0003-no-title.md', '一级标题')

    def test_decision_without_status_is_reported(self):
        (self.fixture.decisions / '0003-no-status.md').write_text(
            '# 0003. 没有状态\n\n- 日期：2026-10-14\n', encoding='utf-8')
        self.assert_rejected('0003-no-status.md', '- 状态：采用')

    def test_two_records_with_the_same_number_are_reported(self):
        # 并行的两个会话拿了同一个编号：索引表盖不住，后合的那条会被挤掉。
        self.fixture.add_decision('0003', 'one-session', '一个会话写的')
        self.fixture.add_decision('0003', 'another-session', '另一个会话写的')
        self.assert_rejected('同一个编号 0003',
                             '0003-one-session.md', '0003-another-session.md')

    def test_decision_title_must_start_with_its_number(self):
        (self.fixture.decisions / '0003-wrong-number.md').write_text(
            '# 0004. 编号对不上\n\n- 状态：采用\n', encoding='utf-8')
        self.assert_rejected('0003-wrong-number.md', '要以“0003. ”开头')

    def test_subsystem_without_a_summary_is_reported(self):
        (self.fixture.subsystems / 'bare.md').write_text('# 没有摘要\n\n正文。\n', encoding='utf-8')
        self.assert_rejected('bare.md', '> 一句话')

    def test_missing_markers_are_reported(self):
        (self.fixture.decisions / 'README.md').write_text('# 架构决定记录\n\n没有标记。\n', encoding='utf-8')
        self.assert_rejected('docs-index:begin')

    def test_a_pipe_in_a_title_is_reported(self):
        self.fixture.add_decision('0003', 'pipe', '带 | 竖线的标题')
        self.assert_rejected('0003-pipe.md', '里有竖线')


class ThisRepo(unittest.TestCase):
    """守护本仓库：两张索引表必须就是生成出来的那张。"""

    def test_committed_indexes_are_generated(self):
        result = subprocess.run([sys.executable, str(SCRIPT), '--check'],
                                capture_output=True, text=True, cwd=os.sep)
        self.assertEqual(result.returncode, 0,
                         f'仓库里的索引和 docs/ 对不上：{result.stdout}{result.stderr}')

    def test_every_decision_record_is_listed(self):
        listed = (REPO / 'docs' / 'decisions' / 'README.md').read_text(encoding='utf-8')
        for record in sorted((REPO / 'docs' / 'decisions').glob('[0-9][0-9][0-9][0-9]-*.md')):
            self.assertIn(f'({record.name})', listed, f'{record.name} 不在索引里')

    def test_every_subsystem_reference_is_listed(self):
        listed = (REPO / 'docs' / 'subsystems' / 'README.md').read_text(encoding='utf-8')
        for doc in sorted((REPO / 'docs' / 'subsystems').glob('*.md')):
            if doc.name == 'README.md':
                continue
            self.assertIn(f'({doc.name})', listed, f'{doc.name} 不在索引里')


if __name__ == '__main__':
    unittest.main()
