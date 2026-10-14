#!/usr/bin/env python3
"""Generate the docs index tables from the files they list. Standard library only.

每个并行 PR 以前都手动往同一张表的末尾追加一行，于是互相冲突（#45）。现在表由
这里生成：PR 只新增自己那个 Markdown 文件，`make docs-index` 把表重新写出来，
`scripts/check.sh` 里的 `--check` 负责拦住忘记生成的改动。
"""
import argparse
from pathlib import Path
import re
import sys

BEGIN = '<!-- docs-index:begin -->'
END = '<!-- docs-index:end -->'
DECISION_FILE = re.compile(r'^(\d{4})-.+\.md$')
STATUS = re.compile(r'^-\s*状态：\s*(\S.*?)\s*$')
FIX = '跑 `make docs-index`（或 `python3 scripts/docs_index.py --write`）重新生成，不要手改表。'


class Invalid(Exception):
    """文档本身写得不对，光生成解决不了，要作者改文件。"""


def heading(rel, lines):
    for index, line in enumerate(lines):
        if line.startswith('# '):
            return line[2:].strip(), lines[index + 1:]
    raise Invalid(f'{rel}：没有一级标题，索引取不到名字')


def cell(rel, value):
    if '|' in value:
        raise Invalid(f'{rel}：“{value}”里有竖线，Markdown 表格放不下，请改掉')
    return value


def decision_rows(directory, rel_dir):
    rows = []
    seen = {}
    for path in sorted(directory.glob('*.md')):
        name = DECISION_FILE.match(path.name)
        if not name:
            continue
        rel = f'{rel_dir}/{path.name}'
        number = name.group(1)
        # 两个会话拿了同一个编号，是索引表盖不住的问题：谁后合谁的记录就被挤出一行。
        if number in seen:
            raise Invalid(f'{rel} 和 {seen[number]} 用了同一个编号 {number}，'
                          f'后写的那条改成下一个没人用的编号')
        seen[number] = rel
        text = path.read_text(encoding='utf-8')
        title, _ = heading(rel, text.splitlines())
        prefix = f'{number}. '
        if not title.startswith(prefix):
            raise Invalid(f'{rel}：一级标题是“{title}”，要以“{prefix}”开头，和文件名的编号一致')
        status = next((STATUS.match(line).group(1) for line in text.splitlines()
                       if STATUS.match(line)), None)
        if status is None:
            raise Invalid(f'{rel}：没有“- 状态：采用”这一行，索引取不到状态')
        rows.append(f'| [{number}]({path.name}) | {cell(rel, title[len(prefix):])} '
                    f'| {cell(rel, status)} |')
    return ['| 编号 | 标题 | 状态 |', '| --- | --- | --- |', *rows]


def subsystem_rows(directory, rel_dir):
    rows = []
    for path in sorted(directory.glob('*.md')):
        if path.name == 'README.md':
            continue
        rel = f'{rel_dir}/{path.name}'
        title, rest = heading(rel, path.read_text(encoding='utf-8').splitlines())
        # 摘要是一级标题后面第一行有内容的文字，必须是 `> 一句话`。
        first = next((line for line in rest if line.strip()), '')
        summary = first[2:].strip() if first.startswith('> ') else ''
        if not summary:
            raise Invalid(f'{rel}：一级标题下面要有一行 `> 一句话`，说明这个子系统做什么；'
                          f'索引第一列用的就是它')
        rows.append(f'| {cell(rel, summary)} | [{cell(rel, title)}]({path.name}) |')
    return ['| 子系统 | 参考 |', '| --- | --- |', *rows]


def splice(rel, text, table):
    """把表放回 README 的标记之间，标记以外的正文一字不动。"""
    start = text.find(BEGIN)
    end = text.find(END)
    if start < 0 or end < 0 or end < start:
        raise Invalid(f'{rel}：找不到 {BEGIN} 和 {END} 这对标记，生成的表不知道放哪，把这对标记加回去')
    head = text[:start + len(BEGIN)]
    tail = text[end:]
    return f'{head}\n' + '\n'.join(table) + '\n' + tail


def current(rel, text):
    start = text.find(BEGIN)
    end = text.find(END)
    if start < 0 or end < 0 or end < start:
        raise Invalid(f'{rel}：找不到 {BEGIN} 和 {END} 这对标记，'
                      f'表就是放在它们中间的，把这对标记加回去')
    return [line for line in text[start + len(BEGIN):end].splitlines() if line.strip()]


def difference(rel, want, got):
    """说清楚差在哪：少了哪行、多了哪行，还是只是顺序不对。"""
    missing = [line for line in want if line not in got]
    extra = [line for line in got if line not in want]
    if not missing and not extra:
        return [f'{rel} 的索引表顺序不对（行都在，排列和生成的不一样）。']
    report = [f'{rel} 的索引表和目录对不上。']
    report += [f'  少了：{line}' for line in missing]
    report += [f'  多了（文件已经不在，或内容已经改了）：{line}' for line in extra]
    return report


def indexes(repo):
    decisions = repo / 'docs' / 'decisions'
    subsystems = repo / 'docs' / 'subsystems'
    return [
        (decisions / 'README.md', 'docs/decisions/README.md',
         decision_rows(decisions, 'docs/decisions')),
        (subsystems / 'README.md', 'docs/subsystems/README.md',
         subsystem_rows(subsystems, 'docs/subsystems')),
    ]


def check(repo):
    problems = []
    for path, rel, table in indexes(repo):
        got = current(rel, path.read_text(encoding='utf-8'))
        want = [line for line in table if line.strip()]
        if got != want:
            problems += difference(rel, want, got)
    if problems:
        print('\n'.join(problems))
        print(FIX)
        return 1
    return 0


def write(repo):
    # 先把两张表都算出来再落盘：某份文档写得不对时，不留下半张生成的表。
    spliced = [(path, splice(rel, path.read_text(encoding='utf-8'), table))
               for path, rel, table in indexes(repo)]
    for path, text in spliced:
        path.write_text(text, encoding='utf-8')
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description='生成或检查 docs 下的索引表')
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--check', action='store_true', help='只比对，不写文件；对不上退出 1')
    mode.add_argument('--write', action='store_true', help='把生成的表写回两个 README')
    parser.add_argument('--repo', type=Path, default=Path(__file__).resolve().parents[1],
                        help='仓库根目录，默认是这个脚本所在仓库')
    args = parser.parse_args(argv)
    try:
        return check(args.repo) if args.check else write(args.repo)
    except Invalid as problem:
        # 这类问题重新生成一遍也不会好，所以不提示跑生成命令，直接说文件要怎么改。
        print(problem, file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
