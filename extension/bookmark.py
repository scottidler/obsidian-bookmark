
# example python api server
from flask import Flask, request, jsonify
from flask_cors import CORS

app = Flask(__name__)
CORS(app)  # Enable CORS for all routes

@app.route('/bookmark_py', methods=['POST'])
def process_bookmark():
    bookmark = request.json
    print('Received bookmark:', bookmark)
    # Add your processing logic here
    return jsonify({'status': 'Python success'})

if __name__ == '__main__':
    app.run(port=5001)

