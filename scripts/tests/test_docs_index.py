"""The docs index tables are generated; nothing here touches the network or real配置."""
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'docs_index.py'
REPO = Path(__file__).resolve().parents[2]

sys.path.insert(0, str(SCRIPT.parent))
# 索引漂移的修复提示。文档本身写得不对时不该出现它：重新生成一遍不会好，要作者改文件。
from docs_index import FIX as REGENERATE  # noqa: E402

BEGIN = '<!-- docs-index:begin -->'
END = '<!-- docs-index:end -->'
# 表格行里指向本目录某份文档的链接，例如 `| [0011](0011-damai-revision-inspection.md) | … |`。
INDEX_ROW = re.compile(r'^\s*\|.*\]\((?P<target>[^)/]+\.md)\)')

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

    def readme(self, which):
        return (self.decisions if which == 'decisions' else self.subsystems) / 'README.md'

    def append_below_the_table(self, which, text):
        """在生成区之后追加正文，前言和生成区一个字节不动。"""
        path = self.readme(which)
        path.write_text(path.read_text(encoding='utf-8') + text, encoding='utf-8')

    def insert_above_the_table(self, which, text):
        """在 begin 标记之前插入正文。"""
        path = self.readme(which)
        original = path.read_text(encoding='utf-8')
        path.write_text(original.replace(BEGIN, text + BEGIN, 1), encoding='utf-8')

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


STALE_DECISION_ROW = '| [0002](0002-watch-timing.md) | 盯票的定时和轮询（旧标题） | 采用 |\n'
STALE_SUBSYSTEM_ROW = '| 从大麦读场次 | [大麦演出信息查询](damai-event.md) |\n'


class OutsideTheTable(unittest.TestCase):
    """索引只在标记之间。生成区已经列了一份文档，区外再出现它就是第二份索引（#80）。

    这正是 union 合并（`.gitattributes`，决定 0015）留下的样子：另一个分支把自己那一行
    加在它当时的位置，合并后两边的行都保留，旧的那批落到 end 标记之后。`--check` 只比对
    标记之间的内容，所以 PR #40 带着三行重复索引合进了 main 而检查全绿。
    """

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.fixture = Fixture(self.tmp.name)

    def assert_check_reports(self, rel, row):
        result = run(self.fixture.root, '--check')
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 1, f'生成区外的索引行必须让检查失败：{output}')
        self.assertIn(rel, output, f'要说是哪份 README：{output}')
        self.assertIn(row.strip(), output, f'要把区外那一行原样列出来：{output}')
        self.assertIn(REGENERATE, output, f'这一类跑一次生成就修好，要给出命令：{output}')

    def assert_write_removes(self, which, row, left=0):
        """生成之后这一行只剩 `left` 处：表头在生成区里还有一份，旧的索引行一处都不剩。"""
        self.assertEqual(run(self.fixture.root, '--write').returncode, 0, '生成要能收拾干净')
        text = self.fixture.readme(which).read_text(encoding='utf-8')
        self.assertEqual(text.count(row.strip()), left,
                         f'生成区外的索引行要被删掉：{text}')
        self.assertEqual(run(self.fixture.root, '--check').returncode, 0,
                         '生成之后检查必须通过')

    def test_a_decision_row_below_the_table_fails(self):
        self.fixture.append_below_the_table('decisions', STALE_DECISION_ROW)
        self.assert_check_reports('docs/decisions/README.md', STALE_DECISION_ROW)
        self.assert_write_removes('decisions', STALE_DECISION_ROW)

    def test_a_subsystem_row_below_the_table_fails(self):
        self.fixture.append_below_the_table('subsystems', STALE_SUBSYSTEM_ROW)
        self.assert_check_reports('docs/subsystems/README.md', STALE_SUBSYSTEM_ROW)
        self.assert_write_removes('subsystems', STALE_SUBSYSTEM_ROW)

    def test_a_row_above_the_table_fails(self):
        self.fixture.insert_above_the_table('decisions', STALE_DECISION_ROW + '\n')
        self.assert_check_reports('docs/decisions/README.md', STALE_DECISION_ROW)
        self.assert_write_removes('decisions', STALE_DECISION_ROW)

    def test_a_second_copy_of_the_header_below_the_table_fails(self):
        header = '| 编号 | 标题 | 状态 |\n'
        self.fixture.append_below_the_table('decisions', header)
        self.assert_check_reports('docs/decisions/README.md', header)
        self.assert_write_removes('decisions', header, left=1)

    def test_the_whole_stale_table_below_the_table_is_removed(self):
        # PR #40 合进来的就是这个样子：一整段旧表跟在 end 标记后面。
        self.fixture.append_below_the_table(
            'decisions',
            STALE_DECISION_ROW
            + '| [0001](0001-damai-event-data-source.md) | 演出信息从大麦详情页获取 | 采用 |\n')
        self.assert_check_reports('docs/decisions/README.md', STALE_DECISION_ROW)
        self.assert_write_removes('decisions', STALE_DECISION_ROW)
        rows = [line for line in self.fixture.decisions_index().splitlines()
                if line.startswith('| [')]
        self.assertEqual(len(rows), 2, f'每份记录只剩索引里的那一行：{rows}')

    def test_prose_and_unrelated_tables_around_the_table_are_kept(self):
        # 正常的前言/后记：带链接的句子、指向记录的行内链接、和索引无关的表格，都要留着。
        keep = ('\n## 后记\n\n'
                '最新一条是 [0002](0002-watch-timing.md)，写法见 [前言](#架构决定记录)。\n\n'
                '| 角色 | 读哪里 |\n| --- | --- |\n'
                '| 实现者 | [代码规范](../code-standards.md) |\n')
        self.fixture.append_below_the_table('decisions', keep)
        result = run(self.fixture.root, '--check')
        self.assertEqual(result.returncode, 0,
                         f'前言后记不是索引行，不该报：{result.stdout}{result.stderr}')
        before = self.fixture.decisions_index()
        self.assertEqual(run(self.fixture.root, '--write').returncode, 0)
        self.assertEqual(self.fixture.decisions_index(), before,
                         '生成不能动标记以外的正文')

    def test_two_begin_markers_are_reported(self):
        # union 合并也可能把标记本身留下两份，那时「生成区」是哪一段就说不清了。
        self.fixture.append_below_the_table('decisions', f'\n{BEGIN}\n')
        for mode in ('--check', '--write'):
            result = run(self.fixture.root, mode)
            output = result.stdout + result.stderr
            self.assertEqual(result.returncode, 1, f'{mode} 要拒绝重复的标记：{output}')
            self.assertIn('docs-index:begin', output, f'{mode} 要说是哪对标记：{output}')


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

    def test_no_index_row_sits_outside_the_generated_block(self):
        """两张表各只有一份。这里的判断不走生成器，省得生成器的规则回归时一起失明（#80）。"""
        for directory in ('decisions', 'subsystems'):
            readme = REPO / 'docs' / directory / 'README.md'
            found = []
            inside = False
            for number, line in enumerate(readme.read_text(encoding='utf-8').splitlines(), 1):
                if line.strip() in (BEGIN, END):
                    inside = line.strip() == BEGIN
                    continue
                row = INDEX_ROW.match(line)
                if not inside and row and (readme.parent / row.group('target')).exists():
                    found.append(f'第 {number} 行 {line.strip()}')
            self.assertEqual(found, [],
                             f'docs/{directory}/README.md 在生成区外还有索引行，'
                             f'读者会看到两份索引：' + '；'.join(found))

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
