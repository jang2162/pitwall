-- A comment.
CREATE TABLE projects (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL DEFAULT 'untitled',
  ratio REAL
);

SELECT p.id, count(*) AS total
FROM projects AS p
WHERE p.name LIKE '%wall' AND p.ratio > 1.5
GROUP BY p.id
ORDER BY total DESC;
