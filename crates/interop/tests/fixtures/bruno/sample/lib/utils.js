const fs = require('fs');
const path = require('path');

exports.image = function (name) {
  return fs.readFileSync(path.join(__dirname, 'images', name)).toString('base64');
};
